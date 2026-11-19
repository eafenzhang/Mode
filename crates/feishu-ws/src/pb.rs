//! protobuf `pbbp2.Frame` 手写编解码（wire 事实：spec `docs/specs/bots-im-transport.md`
//! §5.2；SDK `lib/index.js` @1.61.1 `Frame.encode/decode` :84826-84929）。
//!
//! 字段：1 `SeqID` uint64、2 `LogID` uint64、3 `service` int32、4 `method` int32、
//! 5 repeated `Header{1 key,2 value}`、6 `payloadEncoding` string、7 `payloadType`
//! string、8 `payload` bytes、9 `LogIDNew` string（可选）；decode required =
//! SeqID/LogID/service/method，未知字段按 wire type 跳过——与 SDK 逐条对齐。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq_id: u64,
    pub log_id: u64,
    pub service: i32,
    pub method: i32,
    pub headers: Vec<Header>,
    pub payload_encoding: Option<String>,
    pub payload_type: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub log_id_new: Option<String>,
}

#[derive(Debug)]
pub enum PbError {
    Truncated,
    MissingField(&'static str),
}

impl std::fmt::Display for PbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(formatter, "pbbp2: truncated buffer"),
            Self::MissingField(name) => write!(formatter, "pbbp2: missing required '{name}'"),
        }
    }
}

impl std::error::Error for PbError {}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn write_tag(out: &mut Vec<u8>, field: u64, wire: u64) {
    write_varint(out, (field << 3) | wire);
}

fn write_len_delimited(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    write_tag(out, field, 2);
    write_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, PbError> {
    let mut result = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*pos).ok_or(PbError::Truncated)?;
        *pos += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(PbError::Truncated);
        }
    }
}

fn skip_wire(bytes: &[u8], pos: &mut usize, wire: u64) -> Result<(), PbError> {
    match wire {
        0 => {
            read_varint(bytes, pos)?;
        }
        1 => {
            if *pos + 8 > bytes.len() {
                return Err(PbError::Truncated);
            }
            *pos += 8;
        }
        2 => {
            let len = read_varint(bytes, pos)? as usize;
            let end = *pos + len;
            if end > bytes.len() {
                return Err(PbError::Truncated);
            }
            *pos = end;
        }
        5 => {
            if *pos + 4 > bytes.len() {
                return Err(PbError::Truncated);
            }
            *pos += 4;
        }
        _ => return Err(PbError::Truncated),
    }
    Ok(())
}

/// 与 SDK `Frame.encode`（:84826-84844）逐字段对齐。
pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    write_tag(&mut out, 1, 0);
    write_varint(&mut out, frame.seq_id);
    write_tag(&mut out, 2, 0);
    write_varint(&mut out, frame.log_id);
    // int32 的负值按 protobuf 语义编成 10 字节补码 varint；实际取值恒为小非负数。
    write_tag(&mut out, 3, 0);
    write_varint(&mut out, frame.service as i64 as u64);
    write_tag(&mut out, 4, 0);
    write_varint(&mut out, frame.method as i64 as u64);
    for header in &frame.headers {
        let mut content = Vec::with_capacity(header.key.len() + header.value.len() + 4);
        write_len_delimited(&mut content, 1, header.key.as_bytes());
        write_len_delimited(&mut content, 2, header.value.as_bytes());
        write_len_delimited(&mut out, 5, &content);
    }
    if let Some(value) = &frame.payload_encoding {
        write_len_delimited(&mut out, 6, value.as_bytes());
    }
    if let Some(value) = &frame.payload_type {
        write_len_delimited(&mut out, 7, value.as_bytes());
    }
    if let Some(value) = &frame.payload {
        write_len_delimited(&mut out, 8, value);
    }
    if let Some(value) = &frame.log_id_new {
        write_len_delimited(&mut out, 9, value.as_bytes());
    }
    out
}

/// 与 SDK `Frame.decode`（:84871-84929）逐条对齐：未知字段跳过；required =
/// SeqID/LogID/service/method。
pub fn decode(bytes: &[u8]) -> Result<Frame, PbError> {
    let mut pos = 0usize;
    let mut frame = Frame {
        seq_id: 0,
        log_id: 0,
        service: 0,
        method: 0,
        headers: Vec::new(),
        payload_encoding: None,
        payload_type: None,
        payload: None,
        log_id_new: None,
    };
    let mut seen_seq = false;
    let mut seen_log = false;
    let mut seen_service = false;
    let mut seen_method = false;
    while pos < bytes.len() {
        let tag = read_varint(bytes, &mut pos)?;
        let field = tag >> 3;
        let wire = tag & 7;
        match field {
            1 => {
                frame.seq_id = read_varint(bytes, &mut pos)?;
                seen_seq = true;
            }
            2 => {
                frame.log_id = read_varint(bytes, &mut pos)?;
                seen_log = true;
            }
            3 => {
                frame.service = read_varint(bytes, &mut pos)? as i64 as i32;
                seen_service = true;
            }
            4 => {
                frame.method = read_varint(bytes, &mut pos)? as i64 as i32;
                seen_method = true;
            }
            5 => {
                let len = read_varint(bytes, &mut pos)? as usize;
                let end = pos + len;
                if end > bytes.len() {
                    return Err(PbError::Truncated);
                }
                let content = &bytes[pos..end];
                pos = end;
                let mut inner = 0usize;
                let mut key = String::new();
                let mut value = String::new();
                let mut seen_key = false;
                let mut seen_value = false;
                while inner < content.len() {
                    let tag = read_varint(content, &mut inner)?;
                    match tag >> 3 {
                        1 => {
                            let len = read_varint(content, &mut inner)? as usize;
                            key =
                                String::from_utf8_lossy(&content[inner..inner + len]).into_owned();
                            inner += len;
                            seen_key = true;
                        }
                        2 => {
                            let len = read_varint(content, &mut inner)? as usize;
                            value =
                                String::from_utf8_lossy(&content[inner..inner + len]).into_owned();
                            inner += len;
                            seen_value = true;
                        }
                        _ => skip_wire(content, &mut inner, tag & 7)?,
                    }
                }
                if !seen_key || !seen_value {
                    return Err(PbError::MissingField("Header.key/value"));
                }
                frame.headers.push(Header { key, value });
            }
            6 => {
                let len = read_varint(bytes, &mut pos)? as usize;
                let end = pos + len;
                if end > bytes.len() {
                    return Err(PbError::Truncated);
                }
                frame.payload_encoding =
                    Some(String::from_utf8_lossy(&bytes[pos..end]).into_owned());
                pos = end;
            }
            7 => {
                let len = read_varint(bytes, &mut pos)? as usize;
                let end = pos + len;
                if end > bytes.len() {
                    return Err(PbError::Truncated);
                }
                frame.payload_type = Some(String::from_utf8_lossy(&bytes[pos..end]).into_owned());
                pos = end;
            }
            8 => {
                let len = read_varint(bytes, &mut pos)? as usize;
                let end = pos + len;
                if end > bytes.len() {
                    return Err(PbError::Truncated);
                }
                frame.payload = Some(bytes[pos..end].to_vec());
                pos = end;
            }
            9 => {
                let len = read_varint(bytes, &mut pos)? as usize;
                let end = pos + len;
                if end > bytes.len() {
                    return Err(PbError::Truncated);
                }
                frame.log_id_new = Some(String::from_utf8_lossy(&bytes[pos..end]).into_owned());
                pos = end;
            }
            _ => skip_wire(bytes, &mut pos, wire)?,
        }
    }
    if !seen_seq {
        return Err(PbError::MissingField("SeqID"));
    }
    if !seen_log {
        return Err(PbError::MissingField("LogID"));
    }
    if !seen_service {
        return Err(PbError::MissingField("service"));
    }
    if !seen_method {
        return Err(PbError::MissingField("method"));
    }
    Ok(frame)
}

pub fn header_value<'a>(frame: &'a Frame, key: &str) -> Option<&'a str> {
    frame
        .headers
        .iter()
        .find(|header| header.key == key)
        .map(|header| header.value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ping_control_frame() -> Frame {
        Frame {
            seq_id: 0,
            log_id: 0,
            service: 3,
            method: 0, // control
            headers: vec![Header {
                key: "type".into(),
                value: "ping".into(),
            }],
            payload_encoding: None,
            payload_type: None,
            payload: None,
            log_id_new: None,
        }
    }

    #[test]
    fn ping_frame_matches_hand_computed_bytes() {
        // 08 00 | 10 00 | 18 03 | 20 00 | 2a 0c 0a 04 "type" 12 04 "ping"
        // Header 内容 = tag+len+"type"(6) + tag+len+"ping"(6) = 12 字节（0x0c）。
        let expected: Vec<u8> = vec![
            0x08, 0x00, 0x10, 0x00, 0x18, 0x03, 0x20, 0x00, 0x2a, 0x0c, 0x0a, 0x04, 0x74, 0x79,
            0x70, 0x65, 0x12, 0x04, 0x70, 0x69, 0x6e, 0x67,
        ];
        assert_eq!(encode(&ping_control_frame()), expected);
        assert_eq!(decode(&expected).unwrap(), ping_control_frame());
    }

    #[test]
    fn round_trips_all_fields() {
        let frame = Frame {
            seq_id: 70000,
            log_id: 12345678901,
            service: -1, // int32 负值按 10 字节 varint 补码编解码
            method: 1,
            headers: vec![
                Header {
                    key: "type".into(),
                    value: "event".into(),
                },
                Header {
                    key: "message_id".into(),
                    value: "m-1".into(),
                },
                Header {
                    key: "sum".into(),
                    value: "2".into(),
                },
                Header {
                    key: "seq".into(),
                    value: "0".into(),
                },
            ],
            payload_encoding: Some("json".into()),
            payload_type: Some("application/json".into()),
            payload: Some(vec![1, 2, 3, 250]),
            log_id_new: Some("log-new".into()),
        };
        assert_eq!(decode(&encode(&frame)).unwrap(), frame);
    }

    #[test]
    fn unknown_fields_are_skipped() {
        // 已知帧尾部追加未知字段：15 号 wire 0（varint）与 16 号 wire 2（length-delimited）。
        let mut extended = encode(&ping_control_frame());
        write_tag(&mut extended, 15, 0);
        write_varint(&mut extended, 7);
        write_tag(&mut extended, 16, 2);
        write_varint(&mut extended, 3);
        extended.extend_from_slice(b"abc");
        let decoded = decode(&extended).unwrap();
        assert_eq!(decoded, ping_control_frame());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        // 只有 SeqID：缺 LogID/service/method。
        let bytes = [0x08, 0x01];
        assert!(matches!(
            decode(&bytes),
            Err(PbError::MissingField("LogID"))
        ));
    }
}
