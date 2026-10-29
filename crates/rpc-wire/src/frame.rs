//! 13 字节协议消息头（TS `ProtocolMessage` / `writeProtocolMessage`）。
//!
//! `type(1) + id(4 大端) + ack(4 大端) + length(4 大端)`，随后是 `length` 字节的 payload。
//! 只有 `Regular` 会被协议层向上抛；未知 type 在 TS 侧是「读出来再忽略」，这里按原始
//! `u8` 保存并原样回写，不做枚举校验。

use crate::codec::{DecodeError, Reader};

pub const HEADER_SIZE: usize = 13;

pub mod message_type {
    pub const NONE: u8 = 0;
    pub const REGULAR: u8 = 1;
    pub const CONTROL: u8 = 2;
    pub const ACK: u8 = 3;
    pub const DISCONNECT: u8 = 5;
    pub const REPLAY_REQUEST: u8 = 6;
    pub const PAUSE: u8 = 7;
    pub const RESUME: u8 = 8;
    pub const KEEP_ALIVE: u8 = 9;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolMessage {
    pub message_type: u8,
    pub id: u32,
    pub ack: u32,
    pub data: Vec<u8>,
}

/// 编码一条完整的协议消息（头 + payload）。
pub fn encode_message(message: &ProtocolMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + message.data.len());
    out.push(message.message_type);
    out.extend_from_slice(&message.id.to_be_bytes());
    out.extend_from_slice(&message.ack.to_be_bytes());
    out.extend_from_slice(&(message.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&message.data);
    out
}

/// 解码一条协议消息，返回 `(消息, 消费字节数)`。
///
/// 调用方可在流式缓冲上循环调用：只有 `HEADER_SIZE + length` 都到齐才应消费。
pub fn decode_message(bytes: &[u8]) -> Result<(ProtocolMessage, usize), DecodeError> {
    let mut reader = Reader::new(bytes);
    let header = reader.read(HEADER_SIZE)?;
    let message_type = header[0];
    let id = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    let ack = u32::from_be_bytes([header[5], header[6], header[7], header[8]]);
    let length = u32::from_be_bytes([header[9], header[10], header[11], header[12]]) as usize;
    let data = reader.read(length)?.to_vec();
    Ok((
        ProtocolMessage {
            message_type,
            id,
            ack,
            data,
        },
        HEADER_SIZE + length,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_regular_message() {
        let message = ProtocolMessage {
            message_type: message_type::REGULAR,
            id: 0x01020304,
            ack: 0x0a0b0c0d,
            data: vec![1, 2, 3],
        };
        let bytes = encode_message(&message);
        assert_eq!(bytes.len(), HEADER_SIZE + 3);
        assert_eq!(bytes[0], message_type::REGULAR);
        assert_eq!(&bytes[1..5], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&bytes[5..9], &[0x0a, 0x0b, 0x0c, 0x0d]);
        assert_eq!(&bytes[9..13], &[0, 0, 0, 3]);
        let (decoded, consumed) = decode_message(&bytes).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn keeps_unknown_message_types_verbatim() {
        let message = ProtocolMessage {
            message_type: 42,
            id: 1,
            ack: 0,
            data: Vec::new(),
        };
        let bytes = encode_message(&message);
        assert_eq!(decode_message(&bytes).unwrap().0, message);
    }

    #[test]
    fn rejects_truncated_header_and_body() {
        let message = ProtocolMessage {
            message_type: message_type::REGULAR,
            id: 1,
            ack: 0,
            data: vec![9; 4],
        };
        let bytes = encode_message(&message);
        assert!(decode_message(&bytes[..HEADER_SIZE - 1]).is_err());
        assert!(decode_message(&bytes[..bytes.len() - 1]).is_err());
    }
}
