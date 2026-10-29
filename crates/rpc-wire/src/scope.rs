//! scoped 帧与附件控制帧。
//!
//! 真源：`packages/rpc/src/frameMultiplexer.ts`。两种帧都只出现在**页面桥 ↔ Host**这一段：
//!
//! - scoped 帧：`[0xFF][scopeId 4 字节大端][payload]`；base 帧不带前缀，逐字节不变。
//! - 控制帧：`[0xFE][JSON]`，`{"type":"scope-ready"|"scope-detach","scopeId":N}`。
//!
//! 控制帧的 JSON 由本模块按 `type` → `scopeId` 的顺序输出，与 TS `JSON.stringify`
//! （对象字面量插入序）一致；解析侧接受任意键序。

pub const SCOPED_FRAME_MARKER: u8 = 0xff;
pub const CONTROL_FRAME_MARKER: u8 = 0xfe;
pub const SCOPED_FRAME_HEADER_BYTES: usize = 5;
pub const MAX_SCOPE_ID: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    InvalidScopeId(u32),
    InvalidScopedHeader { len: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedFrame<'a> {
    /// base scope：payload 就是整帧。
    Base(&'a [u8]),
    Scoped { scope_id: u32, payload: &'a [u8] },
}

/// 解析一帧。marker 在但不足 5 字节判为畸形（调用方必须丢弃）；其余情况不复制。
pub fn decode_frame(frame: &[u8]) -> Result<DecodedFrame<'_>, ScopeError> {
    if frame.is_empty() || frame[0] != SCOPED_FRAME_MARKER {
        return Ok(DecodedFrame::Base(frame));
    }
    if frame.len() < SCOPED_FRAME_HEADER_BYTES {
        return Err(ScopeError::InvalidScopedHeader { len: frame.len() });
    }
    let scope_id = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
    Ok(DecodedFrame::Scoped {
        scope_id,
        payload: &frame[SCOPED_FRAME_HEADER_BYTES..],
    })
}

/// 编码 scoped 帧。scope id 0 保留给 base，非法值直接报错。
pub fn encode_scoped_frame(scope_id: u32, payload: &[u8]) -> Result<Vec<u8>, ScopeError> {
    if scope_id == 0 {
        return Err(ScopeError::InvalidScopeId(scope_id));
    }
    let mut out = Vec::with_capacity(SCOPED_FRAME_HEADER_BYTES + payload.len());
    out.push(SCOPED_FRAME_MARKER);
    out.extend_from_slice(&scope_id.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentControl {
    ScopeReady(u32),
    ScopeDetach(u32),
}

impl AttachmentControl {
    pub fn scope_id(&self) -> u32 {
        match self {
            Self::ScopeReady(scope_id) | Self::ScopeDetach(scope_id) => *scope_id,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::ScopeReady(_) => "scope-ready",
            Self::ScopeDetach(_) => "scope-detach",
        }
    }
}

/// 编码控制帧：`[0xFE]{"type":…,"scopeId":…}`（UTF-8）。
pub fn encode_control_frame(control: &AttachmentControl) -> Result<Vec<u8>, ScopeError> {
    let scope_id = control.scope_id();
    if scope_id == 0 {
        return Err(ScopeError::InvalidScopeId(scope_id));
    }
    let json = format!(
        "{{\"type\":\"{}\",\"scopeId\":{scope_id}}}",
        control.kind()
    );
    let mut out = Vec::with_capacity(1 + json.len());
    out.push(CONTROL_FRAME_MARKER);
    out.extend_from_slice(json.as_bytes());
    Ok(out)
}

/// 解析控制帧；不是控制帧或形状不合法都返回 `None`（与 TS 的 `null` 同义）。
pub fn decode_control_frame(frame: &[u8]) -> Option<AttachmentControl> {
    if frame.len() < 2 || frame[0] != CONTROL_FRAME_MARKER {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_slice(&frame[1..]).ok()?;
    let object = parsed.as_object()?;
    let kind = object.get("type")?.as_str()?;
    let scope_id = object.get("scopeId")?.as_u64()?;
    if scope_id == 0 || scope_id > u64::from(MAX_SCOPE_ID) {
        return None;
    }
    let scope_id = scope_id as u32;
    match kind {
        "scope-ready" => Some(AttachmentControl::ScopeReady(scope_id)),
        "scope-detach" => Some(AttachmentControl::ScopeDetach(scope_id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_frames_pass_through_unprefixed() {
        assert_eq!(decode_frame(&[]).unwrap(), DecodedFrame::Base(&[]));
        assert_eq!(
            decode_frame(&[0x04, 0x04]).unwrap(),
            DecodedFrame::Base(&[0x04, 0x04])
        );
    }

    #[test]
    fn scoped_frames_round_trip() {
        let encoded = encode_scoped_frame(7, &[1, 2, 3]).unwrap();
        assert_eq!(encoded, vec![0xff, 0, 0, 0, 7, 1, 2, 3]);
        assert_eq!(
            decode_frame(&encoded).unwrap(),
            DecodedFrame::Scoped {
                scope_id: 7,
                payload: &[1, 2, 3]
            }
        );
    }

    #[test]
    fn rejects_short_scoped_header_and_zero_scope() {
        assert!(matches!(
            decode_frame(&[0xff, 0, 0]),
            Err(ScopeError::InvalidScopedHeader { len: 3 })
        ));
        assert!(matches!(
            encode_scoped_frame(0, &[]),
            Err(ScopeError::InvalidScopeId(0))
        ));
    }

    #[test]
    fn control_frames_match_ts_key_order_and_round_trip() {
        let ready = encode_control_frame(&AttachmentControl::ScopeReady(3)).unwrap();
        assert_eq!(
            String::from_utf8(ready[1..].to_vec()).unwrap(),
            "{\"type\":\"scope-ready\",\"scopeId\":3}"
        );
        assert_eq!(
            decode_control_frame(&ready),
            Some(AttachmentControl::ScopeReady(3))
        );

        let detach = encode_control_frame(&AttachmentControl::ScopeDetach(9)).unwrap();
        assert_eq!(
            decode_control_frame(&detach),
            Some(AttachmentControl::ScopeDetach(9))
        );
    }

    #[test]
    fn rejects_malformed_control_frames() {
        assert_eq!(decode_control_frame(&[0xff, b'{']), None);
        assert_eq!(decode_control_frame(&[0xfe, b'{']), None);
        assert_eq!(
            decode_control_frame(&[0xfe, b'{', b'}']),
            None,
            "缺少字段"
        );
        let zero = b"\xfe{\"type\":\"scope-ready\",\"scopeId\":0}".to_vec();
        assert_eq!(decode_control_frame(&zero), None);
        let unknown = b"\xfe{\"type\":\"nope\",\"scopeId\":1}".to_vec();
        assert_eq!(decode_control_frame(&unknown), None);
    }
}
