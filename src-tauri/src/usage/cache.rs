//! 聚合增量缓存的持久化。
//!
//! 每个会话折叠进度（`consumed` 游标 + `last_sample`/`current_model` 归因
//! 游标 + 按日聚合结果）原子落盘到 `$DSH_HOME/storages/usage-stats-cache.json`。
//! 稳态下每次只折叠新事件，避免大会话日志反复全量解压。
//!
//! 缓存损坏或版本不符时静默退回“全新重折”，绝不因此阻断用量读取。

use std::collections::HashMap;
use std::path::PathBuf;

use super::aggregate::{Buckets, FoldState};
use crate::app_state::Config;

const CACHE_VERSION: u64 = 1;
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
                    consumed: s.consumed,
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

    #[test]
    fn roundtrips_fold_state() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut config = Config::load();
        config.dsh_home = root.clone();
        let mut sessions = HashMap::new();
        sessions.insert("s1".to_string(), FoldState::default());
        save(&config, &sessions).unwrap();
        let loaded = load(&config);
        assert!(loaded.contains_key("s1"));
        let _ = std::fs::remove_dir_all(root);
    }
}
