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

/// 从字节偏移起增量解码：只处理 `offset` 之后的**完整** zstd 帧，返回
/// （新增文本, 推进后的安全偏移）。安全偏移只前进到最后一个完整帧的
/// 末尾——撕裂的尾帧（写者正在追加的半帧）本轮直接跳过，文件补全后的
/// 下一轮自然接上，因此无需错误回退路径。伪 magic 的处理与 `read_full`
/// 相同（候选解码失败即按帧内字节跳过）。
pub(crate) fn read_frames_from(path: &Path, offset: u64) -> Result<(String, u64), String> {
    use std::io::Seek as _;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut cursor = 0usize;
    let mut safe = 0usize;
    while cursor + 4 <= raw.len() {
        if raw[cursor..cursor + 4] == ZSTD_MAGIC {
            match decode_one_frame(&raw[cursor..]) {
                Some((text, consumed)) => {
                    if out.len() + text.len() > MAX_TOTAL_DECOMPRESSED {
                        return Err(format!(
                            "会话日志解压总量超过上限（{} MiB）",
                            MAX_TOTAL_DECOMPRESSED / 1024 / 1024
                        ));
                    }
                    out.push_str(&text);
                    cursor += consumed;
                    // 只有完整帧才推进安全偏移；撕裂尾帧留在下一轮
                    safe = cursor;
                    continue;
                }
                None => cursor += 1, // 伪 magic 或撕裂尾帧
            }
        } else {
            cursor += 1;
        }
    }
    Ok((out, offset + safe as u64))
}

/// `offset` 处是否为合法帧边界（紧跟 zstd magic）。增量折叠的偏移只前进
/// 到完整帧末尾，因此合法偏移后必然是下一帧的 magic 或文件尾；不满足即
/// 说明文件被原地重写（长度不变/变长但内容不同），调用方应整段重折。
pub(crate) fn starts_with_frame_magic(path: &Path, offset: u64) -> Result<bool, String> {
    use std::io::Seek as _;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut magic = [0u8; 4];
    use std::io::Read as _;
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(magic == ZSTD_MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(true), // 文件尾
        Err(e) => Err(e.to_string()),
    }
}

/// 列举 `$DSH_HOME/sessions` 下所有含 `session.jsonl.zstd` 的会话目录，
/// 返回 `(session_id, log_path)`。
///
/// dsh 会把会话按启动工作目录分组到**嵌套**目录（实测如
/// `sessions/--D-git-IdleTrigger--/session-<id>/session.jsonl.zstd`，分组名
/// 随 `cwd` 派生、无法硬编码），因此必须递归扫描而不是只扫一层。会话 id
/// 取包含日志文件的直接父目录名（`session-<id>`），与 RPC `session.list`
/// 的 `sessionId` 及插件缓存 key 对齐。
pub(crate) fn list_sessions(config: &Config) -> Vec<(String, PathBuf)> {
    let dir = config.dsh_home().join("sessions");
    let mut out = Vec::new();
    collect_session_logs(&dir, &mut out);
    out
}

/// 递归收集目录树下所有直接包含 `session.jsonl.zstd` 的会话。
fn collect_session_logs(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
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
        } else {
            // 更深层的分组目录：继续向下递归。
            collect_session_logs(&path, out);
        }
    }
}

/// 按会话 id 定位其真实日志路径（递归枚举，支持嵌套分组目录）。
///
/// 供 `live.rs`（实时 tok/s）与 `notify.rs`（任务完成通知）复用，避免各自
/// 用 `sessions/<id>/session.jsonl.zstd` 拼接路径而在嵌套目录下失效。
pub(crate) fn session_log_path(config: &Config, session_id: &str) -> Option<PathBuf> {
    list_sessions(config)
        .into_iter()
        .find(|(id, _)| id == session_id)
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::{list_sessions, read_full, read_full_limited, session_log_path};

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
    fn read_frames_from_decodes_only_complete_frames_after_offset() {
        use super::read_frames_from;
        let path = temp_log("incr");
        let frame1 = zstd::encode_all(
            "one
"
            .as_bytes(),
            3,
        )
        .unwrap();
        let frame2 = zstd::encode_all(
            "two
"
            .as_bytes(),
            3,
        )
        .unwrap();
        let mut stream = frame1.clone();
        stream.extend_from_slice(&frame2);
        std::fs::write(&path, &stream).unwrap();
        // 从 0 读：两帧全出，偏移推进到文件尾
        let (text, off) = read_frames_from(&path, 0).unwrap();
        assert_eq!(
            text,
            "one
two
"
        );
        assert_eq!(off as usize, stream.len());
        // 从 frame1 之后读：只有 two
        let (text, off) = read_frames_from(&path, frame1.len() as u64).unwrap();
        assert_eq!(
            text,
            "two
"
        );
        assert_eq!(off as usize, stream.len());
        // 尾帧撕裂（砍掉一半字节）：安全偏移停在 frame1 末尾，无文本输出
        let torn = &stream[..stream.len() - 4];
        std::fs::write(&path, torn).unwrap();
        let (text, off) = read_frames_from(&path, 0).unwrap();
        assert_eq!(
            text,
            "one
"
        );
        assert_eq!(off as usize, frame1.len());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
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

    #[test]
    fn list_sessions_recurses_into_grouped_dirs_and_ids_by_log_parent() {
        // dsh 会把会话按启动工作目录分组到嵌套目录：
        // sessions/<group>/session-<id>/session.jsonl.zstd。会话 id 取包含
        // 日志的直接父目录名，不能是分组目录名。
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = crate::app_state::Config::load();
        config.dsh_home = root.clone();
        let group = root.join("sessions").join("--D-git-IdleTrigger--");
        std::fs::create_dir_all(group.join("session-aaa")).unwrap();
        std::fs::create_dir_all(group.join("session-bbb")).unwrap();
        for (sid, tag) in [("session-aaa", "a"), ("session-bbb", "b")] {
            std::fs::write(
                group.join(sid).join("session.jsonl.zstd"),
                zstd::encode_all(format!("{tag}\n").as_bytes(), 3).unwrap(),
            )
            .unwrap();
        }
        let sessions = list_sessions(&config);
        let ids: Vec<&str> = sessions.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"session-aaa"));
        assert!(ids.contains(&"session-bbb"));
        assert_eq!(sessions.len(), 2);
        // 路径必须是完整路径，指向分组目录内的真实日志。
        let (_, pa) = sessions
            .iter()
            .find(|(id, _)| id == "session-aaa")
            .unwrap()
            .clone();
        assert!(pa.ends_with("--D-git-IdleTrigger--/session-aaa/session.jsonl.zstd"));
        // 按 id 定位也应回到完整嵌套路径。
        let p = session_log_path(&config, "session-aaa").unwrap();
        assert_eq!(p, pa);
        assert!(session_log_path(&config, "session-none").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
