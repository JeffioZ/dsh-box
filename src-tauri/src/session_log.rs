//! dsh 追加式 zstd 会话日志的尾帧读取。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
const TAIL_WINDOW: u64 = 256 * 1024;
const MAX_FRAME_DECOMPRESSED: usize = 16 * 1024 * 1024;
const MAX_TOTAL_DECOMPRESSED: usize = 32 * 1024 * 1024;

/// dsh 以多个独立 zstd 帧追加会话日志。只回读文件尾部并从后向前尝试帧
/// magic；压缩数据内的伪 magic 或正在写入的半帧会失败并继续尝试更早候选。
///
/// 语义契约：
/// - `Ok(Some(text))`：最近一个完整帧解码成功；
/// - `Ok(None)`：文件为空（尚无帧）；
/// - `Err(_)`：文件打开/读取失败，或尾部窗口内没有任何可解码的完整帧。
pub(crate) fn read_tail_frame(path: &Path) -> Result<Option<String>, String> {
    Ok(read_tail_frames(path, 1)?.into_iter().next())
}

/// 返回最近若干个可解码帧（从新到旧）。通知扫描多帧，避免 turn/end 后又
/// 追加一个很小的状态帧时漏报；实时速率只取第一个即可。
pub(crate) fn read_tail_frames(path: &Path, limit: usize) -> Result<Vec<String>, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let len = file.metadata().map_err(|error| error.to_string())?.len();
    if len == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_WINDOW)))
        .map_err(|error| error.to_string())?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|error| error.to_string())?;
    let mut search_from = buf.len();
    let mut frames = Vec::new();
    for _ in 0..limit.saturating_mul(4).max(8) {
        let Some(rel_start) = (0..search_from.saturating_sub(3))
            .rev()
            .find(|&index| buf[index..index + 4] == ZSTD_MAGIC)
        else {
            break;
        };
        // 帧按顺序追加：候选帧的内容延伸到下一个 magic 之前（或缓冲区
        // 末尾），而不是无脑切到 buf 末尾——尾部正在写入的半帧因此只影响
        // 自身，更早的完整帧仍可解码。
        let frame_end = (rel_start + 4..buf.len().saturating_sub(3))
            .find(|&index| buf[index..index + 4] == ZSTD_MAGIC)
            .unwrap_or(buf.len());
        if let Ok(decoded) =
            zstd::bulk::decompress(&buf[rel_start..frame_end], MAX_FRAME_DECOMPRESSED)
        {
            if let Ok(text) = String::from_utf8(decoded) {
                frames.push(text);
                if frames.len() >= limit
                    || frames.iter().map(String::len).sum::<usize>() >= MAX_TOTAL_DECOMPRESSED
                {
                    break;
                }
            }
        }
        search_from = rel_start;
    }
    if frames.is_empty() {
        Err("会话日志尾部没有可解码的完整 zstd 帧".into())
    } else {
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::read_tail_frame;

    #[test]
    fn extracts_the_last_appended_zstd_frame() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-session-tail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl.zstd");
        let mut stream = zstd::encode_all("first\n".as_bytes(), 3).unwrap();
        stream.extend_from_slice(&zstd::encode_all("second\n".as_bytes(), 3).unwrap());
        std::fs::write(&path, stream).unwrap();
        assert_eq!(read_tail_frame(&path).unwrap().as_deref(), Some("second\n"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn trailing_partial_frame_does_not_hide_earlier_complete_frames() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-session-partial-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl.zstd");
        let mut stream = zstd::encode_all("first\n".as_bytes(), 3).unwrap();
        stream.extend_from_slice(&zstd::encode_all("second\n".as_bytes(), 3).unwrap());
        // 模拟正在写入的半帧：只截取其前半部分追加到文件尾
        let partial = zstd::encode_all("partial-third\n".as_bytes(), 3).unwrap();
        stream.extend_from_slice(&partial[..partial.len() / 2]);
        std::fs::write(&path, stream).unwrap();
        // 半帧只影响自身，更早的完整帧仍可解码
        assert_eq!(read_tail_frame(&path).unwrap().as_deref(), Some("second\n"));
        let _ = std::fs::remove_dir_all(root);
    }
}
