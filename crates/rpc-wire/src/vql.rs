//! VQL（7 位一组的变长整数）编码。
//!
//! 与 TS 实现的唯一差别是**输入域**：TS 里 `value` 是 JS number，编码前按 32 位有符号
//! 语义参与位运算；这里显式先转 `u32`，让负数（如 `-1` → `FF FF FF FF 0F`）得到同一串字节。

use crate::codec::{DecodeError, Reader};

/// 写入一个 VQL 整数。负数按 32 位无符号量编码（`-1` 是 5 字节，不做符号缩短）。
pub fn write_int_vql(out: &mut Vec<u8>, value: i32) {
    let mut remaining = value as u32;
    loop {
        let byte = (remaining & 0x7f) as u8;
        remaining >>= 7;
        if remaining == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// 读取一个 VQL 整数。
///
/// TS 实现在超过 32 位后会发生移位回绕（未定义行为），这里把超过 5 字节的输入判为错误，
/// 不去复制那份回绕。
pub fn read_int_vql(reader: &mut Reader<'_>) -> Result<i32, DecodeError> {
    let start = reader.position();
    let mut value: i32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = reader.read_u8()?;
        value |= i32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DecodeError::VqlTooLong { offset: start })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(value: i32) -> Vec<u8> {
        let mut out = Vec::new();
        write_int_vql(&mut out, value);
        out
    }

    fn round_trip(value: i32) {
        let bytes = encode(value);
        let mut reader = Reader::new(&bytes);
        assert_eq!(read_int_vql(&mut reader).unwrap(), value, "bytes={bytes:02X?}");
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn encodes_documented_examples() {
        assert_eq!(encode(0), vec![0x00]);
        assert_eq!(encode(127), vec![0x7f]);
        assert_eq!(encode(128), vec![0x80, 0x01]);
        // 负数按 32 位无符号量，与 TS 的 `>>> 7` 路径一致。
        assert_eq!(encode(-1), vec![0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn round_trips_int32_boundaries() {
        for value in [i32::MIN, -2, -1, 0, 1, 127, 128, 16383, 16384, i32::MAX] {
            round_trip(value);
        }
    }

    #[test]
    fn rejects_more_than_five_bytes() {
        let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        let mut reader = Reader::new(&bytes);
        assert!(matches!(
            read_int_vql(&mut reader),
            Err(DecodeError::VqlTooLong { .. })
        ));
    }
}
