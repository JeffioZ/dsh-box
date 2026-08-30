//! 运行时下载的共享原语：流式落盘（.part + 大小上限 + 取消 + 进度）与
//! 文件 SHA-256。
//!
//! 此前 Node 归档与应用 exe 各自实现"下载 + 上限 + 失败清理"，SHA-256
//! 流式校验也是两份；改一处漏一处（镜像锚点策略曾只写在 Node 路径）。
//! 本模块不拼装用户文案——返回结构化错误（Transport/Body/Local/Limit/
//! Cancelled），由调用方映射各自的本地化消息与换源策略（Source/Fatal）。

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// 流式下载的结构化错误。Transport/Body 属"换源可能有效"类，Local 是
/// 本地文件系统问题（换源无意义）——与 Node 路径原有的 Source/Fatal
/// 分类一一对应，语义由调用方映射。
#[derive(Debug)]
pub(crate) enum DownloadError {
    /// 建连/响应头阶段失败（携带原始错误文本）。
    Transport(String),
    /// 响应体读取失败（携带原始错误文本）。
    Body(String),
    /// 本地文件写入失败（携带原始错误文本）。
    Local(String),
    /// 内容超过 max_bytes 上限（.part 已清理）。
    Limit,
    /// 取消回调返回 true（.part 已清理）。
    Cancelled,
}

/// 一次流式下载请求。`progress` 仅在响应带 content-length 时被调用，
/// 节流策略（百分比变化 + 200ms）由本模块统一实施。
pub(crate) struct StreamRequest<'a> {
    pub url: &'a str,
    /// 落盘路径（直接写目标文件，调用方自行决定是否先写 .part 再提交）。
    pub path: &'a Path,
    pub max_bytes: u64,
    /// 附加 User-Agent（None 用 ureq 默认）。
    pub user_agent: Option<&'a str>,
    /// (done, total) 进度回调；返回后本模块按节流策略调用。
    pub progress: Option<&'a dyn Fn(u64, u64)>,
    /// 每个读取周期检查；true 即中止并清理。
    pub cancelled: Option<&'a dyn Fn() -> bool>,
}

/// 流式下载到文件：GET → content-length 预检 → 64KB 分块写入（每块检查
/// 取消与累计上限）→ flush + fsync。任何失败路径都会清理半截文件。
pub(crate) fn stream_to_file(req: StreamRequest<'_>) -> Result<(), DownloadError> {
    let mut request = super::download_client().get(req.url);
    if let Some(ua) = req.user_agent {
        request = request.header("User-Agent", ua);
    }
    let resp = request
        .call()
        .map_err(|e| DownloadError::Transport(e.to_string()))?;
    let total: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if total > req.max_bytes {
        return Err(DownloadError::Limit);
    }
    let mut reader = resp.into_body().into_reader();
    // 写入失败/取消/超限时先 drop 句柄再删文件（Windows 文件锁）
    let result = (|| -> Result<(), DownloadError> {
        let mut file =
            std::fs::File::create(req.path).map_err(|e| DownloadError::Local(e.to_string()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut done: u64 = 0;
        let mut last_pct: i64 = -1;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        loop {
            if req.cancelled.is_some_and(|check| check()) {
                return Err(DownloadError::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| DownloadError::Body(e.to_string()))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > req.max_bytes {
                return Err(DownloadError::Limit);
            }
            if let Err(e) = file.write_all(&buf[..n]) {
                return Err(DownloadError::Local(e.to_string()));
            }
            if total > 0 {
                if let Some(progress) = req.progress {
                    let pct = (((done as f64 / total as f64) * 100.0) as i64).min(100);
                    if pct > last_pct && last_emit.elapsed() >= Duration::from_millis(200) {
                        last_pct = pct;
                        last_emit = Instant::now();
                        progress(done, total);
                    }
                }
            }
        }
        if req.cancelled.is_some_and(|check| check()) {
            return Err(DownloadError::Cancelled);
        }
        if let Err(e) = file.sync_all() {
            return Err(DownloadError::Local(e.to_string()));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(req.path);
    }
    result
}

/// 流式计算文件 SHA-256（小写 hex）。
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::sha256_file;

    #[test]
    fn sha256_file_matches_known_vector() {
        let dir = std::env::temp_dir().join(format!("dsh-box-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vector.bin");
        std::fs::write(&path, b"abc").unwrap();
        // SHA-256("abc") 的标准向量
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sha256_file_of_large_buffered_read_is_stable() {
        // 跨 64KB 缓冲边界：128KB 全零
        let dir = std::env::temp_dir().join(format!("dsh-box-sha2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zeros.bin");
        std::fs::write(&path, vec![0u8; 128 * 1024]).unwrap();
        let first = sha256_file(&path).unwrap();
        let second = sha256_file(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
