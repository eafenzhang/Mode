//! RPC 信封：`serialize(header) + serialize(body)`。
//!
//! header 是 `[RequestType, id, channel, name]`（cancel/dispose 只有 `[type, id]`），
//! body 是调用参数或 `undefined`。取值与 TS `channels.shared.ts` 的 `RequestType` 一致。

use crate::codec::{decode_from, encode_into, DecodeError, Reader};
use crate::Value;

pub mod request_type {
    pub const PROMISE: i32 = 100;
    pub const PROMISE_CANCEL: i32 = 101;
    pub const EVENT_LISTEN: i32 = 102;
    pub const EVENT_DISPOSE: i32 = 103;

    pub fn is_request_type(value: i32) -> bool {
        matches!(
            value,
            PROMISE | PROMISE_CANCEL | EVENT_LISTEN | EVENT_DISPOSE
        )
    }
}

pub mod response_type {
    pub const INITIALIZE: i32 = 200;
    pub const PROMISE_SUCCESS: i32 = 201;
    pub const PROMISE_ERROR: i32 = 202;
    pub const PROMISE_ERROR_OBJ: i32 = 203;
    pub const EVENT_FIRE: i32 = 204;

    pub fn is_response_type(value: i32) -> bool {
        matches!(
            value,
            INITIALIZE | PROMISE_SUCCESS | PROMISE_ERROR | PROMISE_ERROR_OBJ | EVENT_FIRE
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestHeader {
    Promise {
        id: i32,
        channel: String,
        method: String,
    },
    PromiseCancel {
        id: i32,
    },
    EventListen {
        id: i32,
        channel: String,
        event: String,
    },
    EventDispose {
        id: i32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcMessage {
    pub header: RequestHeader,
    pub body: Value,
}

/// 服务端回帧：`Initialize` 的 header 只有类型，其余带 `id`。
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseHeader {
    Initialize,
    PromiseSuccess { id: i32 },
    PromiseError { id: i32 },
    PromiseErrorObj { id: i32 },
    EventFire { id: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponse {
    pub header: ResponseHeader,
    pub body: Value,
}

/// 任一方向的 RPC 帧（按 header 首元素区分）。
#[derive(Debug, Clone, PartialEq)]
pub enum RpcFrame {
    Request(RpcMessage),
    Response(RpcResponse),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeError {
    Decode(DecodeError),
    NotAnEnvelope,
    MissingField(&'static str),
    WrongFieldType(&'static str),
    UnknownRequestType(i32),
    UnknownResponseType(i32),
}

impl From<DecodeError> for EnvelopeError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

/// 编码一条 RPC 消息（header + body）。
pub fn encode_rpc_message(message: &RpcMessage) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(&mut out, &header_value(&message.header));
    encode_into(&mut out, &message.body);
    out
}

/// 解码一条 RPC 消息。
///
/// 末尾多余字节按 TS 的行为**忽略**（对端在协议层保证一帧一条消息），不在这里做严格校验。
pub fn decode_rpc_message(payload: &[u8]) -> Result<RpcMessage, EnvelopeError> {
    let mut reader = Reader::new(payload);
    let header = decode_from(&mut reader)?;
    let request = parse_header(&header)?;
    let body = decode_from(&mut reader)?;
    Ok(RpcMessage {
        header: request,
        body,
    })
}

/// 编码一条服务端回帧（header + body）。
pub fn encode_rpc_response(response: &RpcResponse) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(&mut out, &response_header_value(&response.header));
    encode_into(&mut out, &response.body);
    out
}

/// 解码一条服务端回帧。
pub fn decode_rpc_response(payload: &[u8]) -> Result<RpcResponse, EnvelopeError> {
    let mut reader = Reader::new(payload);
    let header = decode_from(&mut reader)?;
    let response = parse_response_header(&header)?;
    let body = decode_from(&mut reader)?;
    Ok(RpcResponse {
        header: response,
        body,
    })
}

/// 按 header 首元素判断方向并解码。
pub fn decode_rpc_frame(payload: &[u8]) -> Result<RpcFrame, EnvelopeError> {
    let mut reader = Reader::new(payload);
    let header = decode_from(&mut reader)?;
    let Value::Array(items) = &header else {
        return Err(EnvelopeError::NotAnEnvelope);
    };
    let Some(Value::Int(kind)) = items.first() else {
        return Err(EnvelopeError::NotAnEnvelope);
    };
    let body = decode_from(&mut reader)?;
    if request_type::is_request_type(*kind) {
        return Ok(RpcFrame::Request(RpcMessage {
            header: parse_header_items(items)?,
            body,
        }));
    }
    if response_type::is_response_type(*kind) {
        return Ok(RpcFrame::Response(RpcResponse {
            header: parse_response_items(items)?,
            body,
        }));
    }
    Err(EnvelopeError::UnknownRequestType(*kind))
}

/// 该帧是否为形状合法的服务端回帧（只看 header）。
pub fn is_rpc_response_frame(payload: &[u8]) -> bool {
    let mut reader = Reader::new(payload);
    let Ok(value) = decode_from(&mut reader) else {
        return false;
    };
    matches!(value, Value::Array(items) if matches!(items.first(), Some(Value::Int(kind)) if response_type::is_response_type(*kind)))
}

/// 该帧是否为形状合法的 RPC 请求（只看 header，不看 body）。
///
/// 与 TS `isRpcRequestFrame` 同口径：解析第一段，要求是 `[RequestType, …]` 且首元素是
/// 四个合法取值之一。解析失败一律返回 `false`，不抛错。
pub fn is_rpc_request_frame(payload: &[u8]) -> bool {
    let mut reader = Reader::new(payload);
    let Ok(value) = decode_from(&mut reader) else {
        return false;
    };
    matches!(value, Value::Array(items) if items.len() >= 2 && first_is_request_type(&items))
}

fn first_is_request_type(items: &[Value]) -> bool {
    matches!(items.first(), Some(Value::Int(kind)) if request_type::is_request_type(*kind))
}

fn header_value(header: &RequestHeader) -> Value {
    match header {
        RequestHeader::Promise {
            id,
            channel,
            method,
        } => Value::Array(vec![
            Value::Int(request_type::PROMISE),
            Value::Int(*id),
            Value::Str(channel.clone()),
            Value::Str(method.clone()),
        ]),
        RequestHeader::PromiseCancel { id } => Value::Array(vec![
            Value::Int(request_type::PROMISE_CANCEL),
            Value::Int(*id),
        ]),
        RequestHeader::EventListen { id, channel, event } => Value::Array(vec![
            Value::Int(request_type::EVENT_LISTEN),
            Value::Int(*id),
            Value::Str(channel.clone()),
            Value::Str(event.clone()),
        ]),
        RequestHeader::EventDispose { id } => Value::Array(vec![
            Value::Int(request_type::EVENT_DISPOSE),
            Value::Int(*id),
        ]),
    }
}

fn parse_header(value: &Value) -> Result<RequestHeader, EnvelopeError> {
    match value {
        Value::Array(items) => parse_header_items(items),
        _ => Err(EnvelopeError::NotAnEnvelope),
    }
}

fn parse_header_items(items: &[Value]) -> Result<RequestHeader, EnvelopeError> {
    let Some(Value::Int(kind)) = items.first() else {
        return Err(EnvelopeError::NotAnEnvelope);
    };
    match *kind {
        request_type::PROMISE => Ok(RequestHeader::Promise {
            id: int_at(items, 1)?,
            channel: str_at(items, 2, "channel")?,
            method: str_at(items, 3, "method")?,
        }),
        request_type::EVENT_LISTEN => Ok(RequestHeader::EventListen {
            id: int_at(items, 1)?,
            channel: str_at(items, 2, "channel")?,
            event: str_at(items, 3, "event")?,
        }),
        request_type::PROMISE_CANCEL => Ok(RequestHeader::PromiseCancel {
            id: int_at(items, 1)?,
        }),
        request_type::EVENT_DISPOSE => Ok(RequestHeader::EventDispose {
            id: int_at(items, 1)?,
        }),
        other => Err(EnvelopeError::UnknownRequestType(other)),
    }
}

fn parse_response_header(value: &Value) -> Result<ResponseHeader, EnvelopeError> {
    match value {
        Value::Array(items) => parse_response_items(items),
        _ => Err(EnvelopeError::NotAnEnvelope),
    }
}

fn parse_response_items(items: &[Value]) -> Result<ResponseHeader, EnvelopeError> {
    let Some(Value::Int(kind)) = items.first() else {
        return Err(EnvelopeError::NotAnEnvelope);
    };
    match *kind {
        response_type::INITIALIZE => Ok(ResponseHeader::Initialize),
        response_type::PROMISE_SUCCESS => Ok(ResponseHeader::PromiseSuccess {
            id: int_at(items, 1)?,
        }),
        response_type::PROMISE_ERROR => Ok(ResponseHeader::PromiseError {
            id: int_at(items, 1)?,
        }),
        response_type::PROMISE_ERROR_OBJ => Ok(ResponseHeader::PromiseErrorObj {
            id: int_at(items, 1)?,
        }),
        response_type::EVENT_FIRE => Ok(ResponseHeader::EventFire {
            id: int_at(items, 1)?,
        }),
        other => Err(EnvelopeError::UnknownResponseType(other)),
    }
}

fn response_header_value(header: &ResponseHeader) -> Value {
    match header {
        ResponseHeader::Initialize => {
            Value::Array(vec![Value::Int(response_type::INITIALIZE)])
        }
        ResponseHeader::PromiseSuccess { id } => Value::Array(vec![
            Value::Int(response_type::PROMISE_SUCCESS),
            Value::Int(*id),
        ]),
        ResponseHeader::PromiseError { id } => Value::Array(vec![
            Value::Int(response_type::PROMISE_ERROR),
            Value::Int(*id),
        ]),
        ResponseHeader::PromiseErrorObj { id } => Value::Array(vec![
            Value::Int(response_type::PROMISE_ERROR_OBJ),
            Value::Int(*id),
        ]),
        ResponseHeader::EventFire { id } => Value::Array(vec![
            Value::Int(response_type::EVENT_FIRE),
            Value::Int(*id),
        ]),
    }
}

fn int_at(items: &[Value], index: usize) -> Result<i32, EnvelopeError> {
    match items.get(index) {
        Some(Value::Int(value)) => Ok(*value),
        Some(_) => Err(EnvelopeError::WrongFieldType("id")),
        None => Err(EnvelopeError::MissingField("id")),
    }
}

fn str_at(items: &[Value], index: usize, field: &'static str) -> Result<String, EnvelopeError> {
    match items.get(index) {
        Some(Value::Str(value)) => Ok(value.clone()),
        Some(_) => Err(EnvelopeError::WrongFieldType(field)),
        None => Err(EnvelopeError::MissingField(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_promise_with_body() {
        let message = RpcMessage {
            header: RequestHeader::Promise {
                id: 7,
                channel: "host".into(),
                method: "describe".into(),
            },
            body: Value::Json("{\"a\":1}".into()),
        };
        let bytes = encode_rpc_message(&message);
        assert_eq!(decode_rpc_message(&bytes).unwrap(), message);
        assert!(is_rpc_request_frame(&bytes));
    }

    #[test]
    fn decodes_cancel_with_undefined_body() {
        let bytes = encode_rpc_message(&RpcMessage {
            header: RequestHeader::PromiseCancel { id: 9 },
            body: Value::Undefined,
        });
        assert_eq!(
            decode_rpc_message(&bytes).unwrap(),
            RpcMessage {
                header: RequestHeader::PromiseCancel { id: 9 },
                body: Value::Undefined,
            }
        );
    }

    #[test]
    fn rejects_non_envelopes_and_unknown_types() {
        let plain = crate::serialize(&Value::Str("hello".into()));
        assert_eq!(
            decode_rpc_message(&plain),
            Err(EnvelopeError::NotAnEnvelope)
        );
        assert!(!is_rpc_request_frame(&plain));

        let unknown = crate::serialize(&Value::Array(vec![Value::Int(999), Value::Int(1)]));
        assert_eq!(
            decode_rpc_message(&unknown),
            Err(EnvelopeError::UnknownRequestType(999))
        );
        assert!(!is_rpc_request_frame(&unknown));
    }

    #[test]
    fn rejects_truncated_header_fields() {
        let truncated = crate::serialize(&Value::Array(vec![
            Value::Int(request_type::PROMISE),
            Value::Int(1),
        ]));
        assert!(matches!(
            decode_rpc_message(&truncated),
            Err(EnvelopeError::MissingField(_))
        ));
    }
}
