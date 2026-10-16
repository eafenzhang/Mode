//! 壳 ↔ Host sidecar 的帧编解码：`[4 字节大端长度][payload]`。
//!
//! 形状取自 M3 Task 0 的 spike（`docs/plans/2026-10-16-m3-port-transport-spike.md` §4.4）：
//! 两侧都**不解释**载荷——它就是 `packages/rpc` 的不透明 RPC 字节，帧只在进程边界上负责
//! 长度分帧。Node 侧有同形状的实现（`packages/desktop/src/host-standalone/stdioFrames.ts`），
//! 跨语言一致性由**两侧各自的用例**守着，而不是靠这句话：
//!
//! - `tests/host_lifecycle.rs::the_shell_frame_path_reaches_the_host_and_gets_a_reply`
//!   用 Rust 的编码器写一帧真请求、读回真应答（Rust 编码 → Node 解码 → Node 编码 → Rust 解码）；
//! - `apps/desktop-tauri/test/portRoundTrip.test.mts` 从 Node 侧驱动产品 Host 走完整往返。
//!
//! 复评 I4(c) 之前，这里只写了「由 portRoundTrip 守住」——但那条用例的壳侧用的是
//! `test/fixtures/stdioFrames.mts`（Task 0 的替身编解码），产品实现改了端序它也照样绿。
//!
//! 读方向必须**带上限**：长度前缀是攻击面，没有上限的 `read_exact` 会让一个畸形长度
//! 把壳的内存吃光。

use std::io::{self, Read, Write};

/// 单帧上限（与 Node 侧同为 64 MiB）。
pub const MAX_FRAME_BYTES: u32 = 64 * 1024 * 1024;

/// 把载荷编成 `[长度][载荷]`。
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);
    framed
}

/// 读一帧。`Ok(None)` 表示对端正常关闭（EOF 落在帧边界上）。
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(header);
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("帧长度 {length} 超过上限 {MAX_FRAME_BYTES}（长度前缀已不可信，流终止）"),
        ));
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// 写一帧并 flush（壳侧写 Host stdin 时不能攒着，否则 RPC 会卡到超时）。
pub fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    writer.write_all(&encode_frame(payload))?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_a_payload() {
        let framed = encode_frame(&[4, 4, 6, 100, 0]);
        let mut cursor = Cursor::new(framed);
        let frame = read_frame(&mut cursor).expect("读帧成功").expect("有帧");
        assert_eq!(frame, vec![4, 4, 6, 100, 0]);
        assert!(read_frame(&mut cursor).expect("EOF 是正常结束").is_none());
    }

    #[test]
    fn reads_empty_payload_as_a_frame() {
        let mut cursor = Cursor::new(encode_frame(&[]));
        assert_eq!(
            read_frame(&mut cursor).expect("读帧成功"),
            Some(Vec::new()),
            "空载荷是合法帧（由 Host 侧的形状校验决定要不要丢），不是 EOF"
        );
    }

    #[test]
    fn rejects_length_prefix_above_limit() {
        let mut bytes = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 8]);
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("必须拒绝超限长度");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn treats_truncated_header_as_clean_eof() {
        let mut cursor = Cursor::new(vec![0u8, 0u8]);
        assert!(read_frame(&mut cursor).expect("半截长度前缀按对端关闭处理").is_none());
    }
}
