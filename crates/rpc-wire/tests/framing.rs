//! 分帧、协议消息头与 RPC 信封的黄金字节对拍
//! （`scripts/generate-rpc-wire-fixtures.ts` 产出 `fixtures/frames.json`）。

use rpc_wire::{
    decode_control_frame, decode_message, decode_rpc_frame, decode_rpc_message,
    decode_rpc_response, decode_scope_frame, encode_control_frame, encode_message,
    encode_rpc_message, encode_rpc_response, encode_scoped_frame, encode_stdio_frame,
    is_rpc_request_frame, is_rpc_response_frame, request_type, DecodedFrame, EnvelopeError,
    RequestHeader, ResponseHeader, RpcFrame, StdioFrameDecoder, Value,
};

#[derive(serde::Deserialize)]
struct Fixtures {
    messages: Vec<HexEntry>,
    stdio: Vec<StdioEntry>,
    envelopes: Vec<HexEntry>,
    responses: Vec<HexEntry>,
    scopes: Vec<ScopeEntry>,
}

#[derive(serde::Deserialize)]
struct ScopeEntry {
    name: String,
    kind: String,
    hex: String,
}

#[derive(serde::Deserialize)]
struct HexEntry {
    name: String,
    hex: String,
}

#[derive(serde::Deserialize)]
struct StdioEntry {
    name: String,
    #[serde(rename = "payloadHex")]
    payload_hex: String,
    hex: String,
}

fn load() -> Fixtures {
    serde_json::from_str(include_str!("fixtures/frames.json")).expect("frames.json 必须是合法 JSON")
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("十六进制字符"))
        .collect()
}

#[test]
fn protocol_messages_round_trip_byte_for_byte() {
    let fixtures = load();
    assert_eq!(fixtures.messages.len(), 4);
    for entry in &fixtures.messages {
        let bytes = from_hex(&entry.hex);
        let (message, consumed) =
            decode_message(&bytes).unwrap_or_else(|error| panic!("{}: {error:?}", entry.name));
        assert_eq!(consumed, bytes.len(), "{} 的消费长度应与帧长一致", entry.name);
        assert_eq!(encode_message(&message), bytes, "{} 再编码不一致", entry.name);
    }
}

#[test]
fn stdio_frames_decode_byte_by_byte() {
    let fixtures = load();
    for entry in &fixtures.stdio {
        let expected_payload = from_hex(&entry.payload_hex);
        assert_eq!(
            encode_stdio_frame(&expected_payload),
            from_hex(&entry.hex),
            "{} 的编码与 TS 不一致",
            entry.name
        );

        let mut decoder = StdioFrameDecoder::new();
        let mut frames = Vec::new();
        for byte in from_hex(&entry.hex) {
            frames.extend(decoder.push(&[byte]).unwrap());
        }
        assert_eq!(frames.len(), 1, "{} 应解出一帧", entry.name);
        assert_eq!(frames[0], expected_payload, "{} payload 不一致", entry.name);
        assert_eq!(decoder.pending_len(), 0);
    }
}

#[test]
fn rpc_envelopes_round_trip_and_pass_shape_check() {
    let fixtures = load();
    assert_eq!(fixtures.envelopes.len(), 5);
    for entry in &fixtures.envelopes {
        let bytes = from_hex(&entry.hex);
        let message =
            decode_rpc_message(&bytes).unwrap_or_else(|error| panic!("{}: {error:?}", entry.name));
        assert_eq!(
            encode_rpc_message(&message),
            bytes,
            "{} 再编码不一致",
            entry.name
        );
        assert!(is_rpc_request_frame(&bytes), "{} 应被判定为合法请求帧", entry.name);
    }
}

#[test]
fn envelope_fixtures_cover_every_request_type() {
    let fixtures = load();
    let headers: Vec<RequestHeader> = fixtures
        .envelopes
        .iter()
        .map(|entry| decode_rpc_message(&from_hex(&entry.hex)).unwrap().header)
        .collect();
    for (label, matched) in [
        (
            "promise",
            headers
                .iter()
                .any(|header| matches!(header, RequestHeader::Promise { .. })),
        ),
        (
            "promise-cancel",
            headers
                .iter()
                .any(|header| matches!(header, RequestHeader::PromiseCancel { .. })),
        ),
        (
            "event-listen",
            headers
                .iter()
                .any(|header| matches!(header, RequestHeader::EventListen { .. })),
        ),
        (
            "event-dispose",
            headers
                .iter()
                .any(|header| matches!(header, RequestHeader::EventDispose { .. })),
        ),
    ] {
        assert!(matched, "黄金文件缺少 {label} 样例");
    }
}

#[test]
fn rpc_responses_round_trip_and_pass_shape_check() {
    let fixtures = load();
    assert_eq!(fixtures.responses.len(), 5);
    for entry in &fixtures.responses {
        let bytes = from_hex(&entry.hex);
        let response =
            decode_rpc_response(&bytes).unwrap_or_else(|error| panic!("{}: {error:?}", entry.name));
        assert_eq!(
            encode_rpc_response(&response),
            bytes,
            "{} 再编码不一致",
            entry.name
        );
        assert!(is_rpc_response_frame(&bytes), "{} 应被判定为合法回帧", entry.name);
        assert!(!is_rpc_request_frame(&bytes));
        assert!(matches!(
            decode_rpc_frame(&bytes).unwrap(),
            RpcFrame::Response(_)
        ));
    }
}

#[test]
fn response_fixtures_cover_every_response_type() {
    let fixtures = load();
    let headers: Vec<ResponseHeader> = fixtures
        .responses
        .iter()
        .map(|entry| decode_rpc_response(&from_hex(&entry.hex)).unwrap().header)
        .collect();
    for (label, matched) in [
        (
            "initialize",
            headers
                .iter()
                .any(|header| matches!(header, ResponseHeader::Initialize)),
        ),
        (
            "promise-success",
            headers
                .iter()
                .any(|header| matches!(header, ResponseHeader::PromiseSuccess { .. })),
        ),
        (
            "promise-error",
            headers
                .iter()
                .any(|header| matches!(header, ResponseHeader::PromiseError { .. })),
        ),
        (
            "promise-error-obj",
            headers
                .iter()
                .any(|header| matches!(header, ResponseHeader::PromiseErrorObj { .. })),
        ),
        (
            "event-fire",
            headers
                .iter()
                .any(|header| matches!(header, ResponseHeader::EventFire { .. })),
        ),
    ] {
        assert!(matched, "黄金文件缺少 {label} 样例");
    }
}

#[test]
fn decode_rpc_frame_separates_requests_from_responses() {
    let fixtures = load();
    for entry in &fixtures.envelopes {
        assert!(matches!(
            decode_rpc_frame(&from_hex(&entry.hex)).unwrap(),
            RpcFrame::Request(_)
        ));
    }
}

#[test]
fn rejects_garbage_frames_without_panicking() {
    assert!(!is_rpc_request_frame(&[]));
    assert!(!is_rpc_request_frame(&[0xff]));
    let unknown = rpc_wire::serialize(&Value::Array(vec![Value::Int(999), Value::Int(1)]));
    assert!(!is_rpc_request_frame(&unknown));
    assert!(matches!(
        decode_rpc_message(&unknown),
        Err(EnvelopeError::UnknownRequestType(999))
    ));
    assert_eq!(request_type::PROMISE, 100);
}

#[test]
fn scoped_and_control_frames_round_trip_byte_for_byte() {
    let fixtures = load();
    assert_eq!(fixtures.scopes.len(), 3);
    for entry in &fixtures.scopes {
        let bytes = from_hex(&entry.hex);
        match entry.kind.as_str() {
            "scoped" => {
                let DecodedFrame::Scoped { scope_id, payload } =
                    decode_scope_frame(&bytes).expect("scoped 帧必须可解析")
                else {
                    panic!("{} 应是 scoped 帧", entry.name);
                };
                assert_eq!(
                    encode_scoped_frame(scope_id, payload).unwrap(),
                    bytes,
                    "{} 再编码不一致",
                    entry.name
                );
            }
            "control" => {
                let control = decode_control_frame(&bytes)
                    .unwrap_or_else(|| panic!("{} 应是控制帧", entry.name));
                assert_eq!(
                    encode_control_frame(&control).unwrap(),
                    bytes,
                    "{} 再编码不一致",
                    entry.name
                );
            }
            other => panic!("未知样例类型 {other}"),
        }
    }
}
