//! 聚合增量缓存的持久化。
//!
//! 每个会话折叠进度（`consumed` 游标 + `last_sample`/`current_model` 归因
//! 游标 + 按日聚合结果）原子落盘到 `$DSH_HOME/storages/usage-stats-cache.json`。
//! 稳态下每次只折叠新事件，避免大会话日志反复全量解压。
//!
//! 缓存损坏或版本不符时静默退回“全新重折”，绝不因此阻断用量读取。

use std::collections::HashMap;
use std::path::PathBuf;

use super::aggregate::{Buckets, CurrentRoute, FoldKind, FoldState};
use crate::app_state::Config;

// v3：FoldState 新增 kind 与 current_route（上游 v0.3）。旧版本缓存按现有
// 语义静默丢弃、全新重折，不做迁移。
const CACHE_VERSION: u64 = 3;
/// 缓存文件带 `dshbox-` 前缀：与参考项目 dsh-usage-stats（其缓存为
/// `$DSH_HOME/storages/usage-stats-cache.json`）隔离，避免两个聚合器读写
/// 同一文件互相覆盖（缓存结构版本不同，同名会互相重置）。
const FILE_NAME: &str = "dshbox-usage-stats-cache.json";

/// 缓存根文件。
pub(crate) fn cache_path(config: &Config) -> PathBuf {
    config.dsh_home().join("storages").join(FILE_NAME)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OnDisk {
    version: u64,
    sessions: HashMap<String, SessionOnDisk>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionOnDisk {
    consumed: u64,
    days: HashMap<String, DayOnDisk>,
    #[serde(default)]
    last_sample: Option<SampleOnDisk>,
    #[serde(default)]
    current_model: Option<String>,
    /// 折叠数据来源（"live" | "persisted"，缺省 persisted——与上游
    /// `parseSession` 的回落口径一致）。
    #[serde(default)]
    kind: String,
    #[serde(default)]
    current_route: Option<RouteOnDisk>,
    /// 上次折叠时的日志文件长度（缺省 0 = 未知，下次必然重折一次）。
    #[serde(default)]
    file_len: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RouteOnDisk {
    provider_id: String,
    model: String,
    #[serde(default)]
    updated_at: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DayOnDisk {
    totals: BucketOnDisk,
    models: HashMap<String, BucketOnDisk>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct BucketOnDisk {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SampleOnDisk {
    key: String,
    day: String,
    model: String,
    buckets: BucketOnDisk,
}

impl From<Buckets> for BucketOnDisk {
    fn from(b: Buckets) -> Self {
        BucketOnDisk {
            input: b.input,
            output: b.output,
            cache_read: b.cache_read,
            cache_write: b.cache_write,
        }
    }
}

impl From<BucketOnDisk> for Buckets {
    fn from(b: BucketOnDisk) -> Self {
        Buckets {
            input: b.input,
            output: b.output,
            cache_read: b.cache_read,
            cache_write: b.cache_write,
        }
    }
}

/// 从缓存加载全部会话折叠状态（不存在/损坏返回空）。
pub(crate) fn load(config: &Config) -> HashMap<String, FoldState> {
    let Ok(text) = std::fs::read_to_string(cache_path(config)) else {
        return HashMap::new();
    };
    let Ok(disk) = serde_json::from_str::<OnDisk>(&text) else {
        return HashMap::new();
    };
    if disk.version != CACHE_VERSION {
        return HashMap::new();
    }
    disk.sessions
        .into_iter()
        .map(|(id, s)| {
            (
                id,
                FoldState {
                    days: s
                        .days
                        .into_iter()
                        .map(|(day, d)| {
                            (
                                day,
                                super::aggregate::DayEntry {
                                    totals: d.totals.into(),
                                    models: d
                                        .models
                                        .into_iter()
                                        .map(|(m, b)| (m, b.into()))
                                        .collect(),
                                },
                            )
                        })
                        .collect(),
                    last_sample: s.last_sample.map(|x| super::aggregate::SampleRef {
                        key: x.key,
                        day: x.day,
                        model: x.model,
                        buckets: x.buckets.into(),
                    }),
                    current_model: s.current_model,
                    current_route: s.current_route.map(|r| CurrentRoute {
                        provider_id: r.provider_id,
                        model: r.model,
                        updated_at: r.updated_at,
                    }),
                    kind: FoldKind::parse(&s.kind),
                    consumed: s.consumed,
                    file_len: s.file_len,
                },
            )
        })
        .collect()
}

/// 把会话折叠状态原子落盘（temp + rename）。失败仅日志，不阻断主流程。
pub(crate) fn save(config: &Config, sessions: &HashMap<String, FoldState>) -> Result<(), String> {
    let path = cache_path(config);
    let parent = path.parent().ok_or("invalid cache path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let disk = OnDisk {
        version: CACHE_VERSION,
        sessions: sessions
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    SessionOnDisk {
                        consumed: s.consumed,
                        days: s
                            .days
                            .iter()
                            .map(|(day, d)| {
                                (
                                    day.clone(),
                                    DayOnDisk {
                                        totals: d.totals.into(),
                                        models: d
                                            .models
                                            .iter()
                                            .map(|(m, b)| (m.clone(), (*b).into()))
                                            .collect(),
                                    },
                                )
                            })
                            .collect(),
                        last_sample: s.last_sample.as_ref().map(|x| SampleOnDisk {
                            key: x.key.clone(),
                            day: x.day.clone(),
                            model: x.model.clone(),
                            buckets: x.buckets.into(),
                        }),
                        current_model: s.current_model.clone(),
                        current_route: s.current_route.as_ref().map(|r| RouteOnDisk {
                            provider_id: r.provider_id.clone(),
                            model: r.model.clone(),
                            updated_at: r.updated_at,
                        }),
                        kind: s.kind.as_str().to_string(),
                        file_len: s.file_len,
                    },
                )
            })
            .collect(),
    };
    let text = serde_json::to_string(&disk).map_err(|e| e.to_string())?;
    crate::app_state::atomic_write(&path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(tag: &str) -> (Config, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-cache-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut config = Config::load();
        config.dsh_home = root.clone();
        (config, root)
    }

    #[test]
    fn roundtrips_fold_state() {
        let (config, root) = temp_config("roundtrip");
        let mut sessions = HashMap::new();
        let state = FoldState {
            consumed: 7,
            file_len: 4096,
            kind: FoldKind::Persisted,
            current_route: Some(CurrentRoute {
                provider_id: "deepseek-official".to_string(),
                model: "deepseek-chat".to_string(),
                updated_at: Some(1_780_000_000_000),
            }),
            ..Default::default()
        };
        sessions.insert("s1".to_string(), state);
        save(&config, &sessions).unwrap();
        let loaded = load(&config);
        let state = loaded.get("s1").unwrap();
        assert_eq!(state.consumed, 7);
        assert_eq!(state.file_len, 4096);
        assert_eq!(state.kind, FoldKind::Persisted);
        let route = state.current_route.as_ref().unwrap();
        assert_eq!(route.provider_id, "deepseek-official");
        assert_eq!(route.model, "deepseek-chat");
        assert_eq!(route.updated_at, Some(1_780_000_000_000));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn drops_cache_from_older_version() {
        // 旧版本缓存不做迁移：静默丢弃，由调用方全新重折。
        let (config, root) = temp_config("v1");
        let path = cache_path(&config);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"sessions":{"s1":{"consumed":7,"days":{}}}}"#,
        )
        .unwrap();
        assert!(load(&config).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
