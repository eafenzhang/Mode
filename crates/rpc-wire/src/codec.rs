//! `serialize` / `deserialize` 与游标读取。
//!
//! 分派顺序与长度口径见 `docs/specs/rpc-wire-format.md`；改动这里之前先改 spec 与 TS 实现。

use crate::tag;
use crate::vql::{read_int_vql, write_int_vql};
use crate::Value;

/// 解码失败的位置与原因。位置是出错的字节偏移，便于对照黄金字节定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEnd {
        offset: usize,
        needed: usize,
        remaining: usize,
    },
    InvalidUtf8 {
        offset: usize,
    },
    VqlTooLong {
        offset: usize,
    },
    NegativeLength {
        offset: usize,
        length: i32,
    },
    UnknownTag {
        offset: usize,
        tag: u8,
    },
    TrailingBytes {
        offset: usize,
        remaining: usize,
    },
}

/// 只读游标。所有读取都做边界检查，越界返回 `UnexpectedEnd` 而不是 panic。
pub struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read(1)?[0])
    }

    pub fn read(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < count {
            return Err(DecodeError::UnexpectedEnd {
                offset: self.position,
                needed: count,
                remaining: self.remaining(),
            });
        }
        let start = self.position;
        self.position += count;
        Ok(&self.bytes[start..self.position])
    }
}

/// 把值编码成线格式字节。
pub fn serialize(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Undefined => out.push(tag::UNDEFINED),
        Value::Str(text) => {
            out.push(tag::STRING);
            write_length(out, text.len());
            out.extend_from_slice(text.as_bytes());
        }
        Value::Bytes(bytes) => {
            out.push(tag::BUFFER);
            write_length(out, bytes.len());
            out.extend_from_slice(bytes);
        }
        Value::Buffer(bytes) => {
            out.push(tag::VS_BUFFER);
            write_length(out, bytes.len());
            out.extend_from_slice(bytes);
        }
        Value::Int(number) => {
            out.push(tag::INT);
            write_int_vql(out, *number);
        }
        Value::Array(items) => {
            out.push(tag::ARRAY);
            write_length(out, items.len());
            for item in items {
                write_value(out, item);
            }
        }
        Value::Json(text) => {
            out.push(tag::OBJECT);
            write_length(out, text.len());
            out.extend_from_slice(text.as_bytes());
        }
    }
}

fn write_length(out: &mut Vec<u8>, length: usize) {
    debug_assert!(
        length <= i32::MAX as usize,
        "payload 长度超出 i32，TS 侧同样会回绕；这属于不会出现的输入"
    );
    write_int_vql(out, length as i32);
}

/// 解码一个值，并要求**整段字节都被消费**。
///
/// 多帧流里一帧的确切边界由外层长度前缀决定，所以「解出一个值」之后还有剩余字节，
/// 说明调用方切错了帧；这里直接报错，避免静默丢字节。
pub fn deserialize(bytes: &[u8]) -> Result<Value, DecodeError> {
    let mut reader = Reader::new(bytes);
    let value = read_value(&mut reader)?;
    if reader.remaining() != 0 {
        return Err(DecodeError::TrailingBytes {
            offset: reader.position(),
            remaining: reader.remaining(),
        });
    }
    Ok(value)
}

fn read_value(reader: &mut Reader<'_>) -> Result<Value, DecodeError> {
    let tag_offset = reader.position();
    let kind = reader.read_u8()?;
    match kind {
        tag::UNDEFINED => Ok(Value::Undefined),
        tag::STRING => {
            let length = read_length(reader)?;
            let start = reader.position();
            let bytes = reader.read(length)?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| DecodeError::InvalidUtf8 { offset: start })?;
            Ok(Value::Str(text.to_owned()))
        }
        tag::BUFFER => {
            let length = read_length(reader)?;
            Ok(Value::Bytes(reader.read(length)?.to_vec()))
        }
        tag::VS_BUFFER => {
            let length = read_length(reader)?;
            Ok(Value::Buffer(reader.read(length)?.to_vec()))
        }
        tag::ARRAY => {
            let length = read_length(reader)?;
            let mut items = Vec::with_capacity(length.min(1024));
            for _ in 0..length {
                items.push(read_value(reader)?);
            }
            Ok(Value::Array(items))
        }
        tag::OBJECT => {
            let length = read_length(reader)?;
            let start = reader.position();
            let bytes = reader.read(length)?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| DecodeError::InvalidUtf8 { offset: start })?;
            Ok(Value::Json(text.to_owned()))
        }
        tag::INT => Ok(Value::Int(read_int_vql(reader)?)),
        other => Err(DecodeError::UnknownTag {
            offset: tag_offset,
            tag: other,
        }),
    }
}

fn read_length(reader: &mut Reader<'_>) -> Result<usize, DecodeError> {
    let offset = reader.position();
    let length = read_int_vql(reader)?;
    if length < 0 {
        return Err(DecodeError::NegativeLength { offset, length });
    }
    Ok(length as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_each_tag() {
        assert_eq!(deserialize(&[tag::UNDEFINED]).unwrap(), Value::Undefined);
        assert_eq!(
            deserialize(&[tag::STRING, 0x02, b'h', b'i']).unwrap(),
            Value::Str("hi".into())
        );
        assert_eq!(
            deserialize(&[tag::BUFFER, 0x01, 0xff]).unwrap(),
            Value::Bytes(vec![0xff])
        );
        assert_eq!(
            deserialize(&[tag::VS_BUFFER, 0x01, 0x00]).unwrap(),
            Value::Buffer(vec![0x00])
        );
        assert_eq!(deserialize(&[tag::INT, 0x01]).unwrap(), Value::Int(1));
        assert_eq!(
            deserialize(&[tag::ARRAY, 0x02, tag::INT, 0x01, tag::STRING, 0x01, b'a']).unwrap(),
            Value::Array(vec![Value::Int(1), Value::Str("a".into())])
        );
        assert_eq!(
            deserialize(&[tag::OBJECT, 0x05, b'[', b'1', b',', b'2', b']']).unwrap(),
            Value::Json("[1,2]".into())
        );
    }

    #[test]
    fn rejects_truncated_and_trailing_bytes() {
        assert!(matches!(
            deserialize(&[tag::STRING, 0x02, b'h']),
            Err(DecodeError::UnexpectedEnd { .. })
        ));
        assert!(matches!(
            deserialize(&[tag::UNDEFINED, 0x00]),
            Err(DecodeError::TrailingBytes { .. })
        ));
        assert!(matches!(
            deserialize(&[0x7f]),
            Err(DecodeError::UnknownTag { tag: 0x7f, .. })
        ));
    }

    #[test]
    fn rejects_negative_length() {
        assert!(matches!(
            deserialize(&[tag::STRING, 0xff, 0xff, 0xff, 0xff, 0x0f]),
            Err(DecodeError::NegativeLength { length: -1, .. })
        ));
    }
}
