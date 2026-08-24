//! 会话日志的全量解码与会话枚举。
//!
//! dsh 以多个独立 zstd 帧**顺序追加**写 `sessions/<id>/session.jsonl.zstd`。
//! 尾帧读取（`session_log.rs`）只覆盖最近 256KB；聚合需要从文件头逐帧解压
//! 拼接出完整事件流。这里单独实现全量读取，避免与尾帧读取的窗口逻辑耦合。

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::app_state::Config;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// 单帧最大解压大小（防御异常输入）。
const MAX_FRAME_DECOMPRESSED: usize = 16 * 1024 * 1024;
/// 全文件解压总量上限（防大日志内存尖峰）。
const MAX_TOTAL_DECOMPRESSED: usize = 512 * 1024 * 1024;

/// 全量解码一个会话日志文件为 multiline 事件文本。
///
/// 实现：顺序扫描文件字节，在每一个 zstd 帧 magic 处尝试解压；成功即把
/// 解压文本追加到输出并跳到帧尾。压缩数据内出现伪 magic 时解压会失败，
/// 该候选被当作帧内字节跳过，继续向后扫描（伪 magic 概率极低）。
pub(crate) fn read_full(path: &Path) -> Result<String, String> {
    read_full_limited(path, MAX_TOTAL_DECOMPRESSED)
}

/// `read_full` 的限量版（上限可注入，便于测试）。
fn read_full_limited(path: &Path, max_total: usize) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut cursor = 0usize;
    while cursor + 4 <= raw.len() {
        if raw[cursor..cursor + 4] == ZSTD_MAGIC {
            // 从当前候选起解压整段剩余？——不，需要确定帧边界。zstd 帧是
            // 自描述的：`zstd::bulk::decompress` 只解一个完整帧，返回帧消耗。
            // 但该 API 不解码到「剩余字节偏移」。这里改用流式解码器以获得
            // 精确帧边界。
            match decode_one_frame(&raw[cursor..]) {
                Some((text, consumed)) => {
                    if out.len() + text.len() > max_total {
                        return Err(format!(
                            "会话日志解压总量超过上限（{} MiB）",
                            max_total / 1024 / 1024
                        ));
                    }
                    if !text.is_empty() {
                        out.push_str(&text);
                    }
                    cursor += consumed;
                    continue;
                }
                None => cursor += 1, // 伪 magic，当作帧内字节
            }
        } else {
            cursor += 1;
        }
    }
    Ok(out)
}

/// 解码从 `bytes` 起始的一个 zstd 帧，返回 (解压文本, 该帧字节长度)。
/// 使用流式解码器并置于 single_frame 模式，读到帧尾即停，可拿到精确
/// 帧边界（`Decoder` 底层 Cursor 的位置就是消耗的原始字节数）。
fn decode_one_frame(bytes: &[u8]) -> Option<(String, usize)> {
    let mut cursor = std::io::Cursor::new(bytes);
    let mut decoder = zstd::stream::read::Decoder::with_buffer(&mut cursor)
        .ok()?
        .single_frame();
    // 整帧解码为字节后一次性 from_utf8（对照 session_log.rs 的整帧范式）：
    // 逐块 from_utf8_lossy 会把跨 64KB 块边界的多字节字符拆成 U+FFFD。
    let mut decoded = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                decoded.extend_from_slice(&buf[..n]);
                if decoded.len() > MAX_FRAME_DECOMPRESSED {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    // 读取结束（帧尾），cursor 停留在该帧之后。
    let consumed = cursor.position() as usize;
    if consumed == 0 {
        return None;
    }
    let text = String::from_utf8(decoded).ok()?;
    Some((text, consumed))
}

/// 列举 `$DSH_HOME/sessions` 下所有含 `session.jsonl.zstd` 的会话目录，
/// 返回 `(session_id, log_path)`。
pub(crate) fn list_sessions(config: &Config) -> Vec<(String, PathBuf)> {
    let dir = config.dsh_home().join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let log = path.join("session.jsonl.zstd");
        if log.is_file() {
            if let Some(id) = path.file_name().and_then(|n| n.to_str()) {
                out.push((id.to_string(), log));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{read_full, read_full_limited};

    fn temp_log(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-full-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root.join("session.jsonl.zstd")
    }

    #[test]
    fn decodes_appended_frames_from_head() {
        let path = temp_log("frames");
        let mut stream = zstd::encode_all("one\n".as_bytes(), 3).unwrap();
        stream.extend_from_slice(&zstd::encode_all("two\n".as_bytes(), 3).unwrap());
        stream.extend_from_slice(&zstd::encode_all("three\n".as_bytes(), 3).unwrap());
        std::fs::write(&path, stream).unwrap();
        assert_eq!(read_full(&path).unwrap(), "one\ntwo\nthree\n");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn decodes_multibyte_char_spanning_read_buffer_boundary() {
        // 「界」为 3 字节 UTF-8，起点放在 64KB 解码缓冲的最后两个字节处，
        // 逐块 lossy 会把它拆成 U+FFFD；整帧 from_utf8 必须原样还原。
        let mut payload = "a".repeat(64 * 1024 - 1);
        payload.push_str("界\n");
        payload.push_str(&"b".repeat(70 * 1024));
        let path = temp_log("utf8");
        std::fs::write(&path, zstd::encode_all(payload.as_bytes(), 3).unwrap()).unwrap();
        assert_eq!(read_full(&path).unwrap(), payload);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn errors_when_total_decompressed_exceeds_cap() {
        let path = temp_log("cap");
        let mut stream = zstd::encode_all("one\n".as_bytes(), 3).unwrap();
        stream.extend_from_slice(&zstd::encode_all("two\n".as_bytes(), 3).unwrap());
        std::fs::write(&path, stream).unwrap();
        // 两帧各 4 字节：上限 6 时第二帧触发报错，上限 8 时完整读出。
        assert!(read_full_limited(&path, 6).is_err());
        assert_eq!(read_full_limited(&path, 8).unwrap(), "one\ntwo\n");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
