//! 插件宿主 sidecar 的 NDJSON 协议（M3 生态域 1 · Task 2 · DR-PH-2）。
//!
//! 一行一帧：请求 / 通知 / 应答 / 错误。方法集合是封闭的（[`Method`]），未知方法直接失败；
//! 解码不猜——同时带 `method` 与 `result` 的帧是非法帧，而不是「随便挑一个」。
//!
//! 协议版本：`host/hello` 里交换；当前唯一支持的值是 [`PROTOCOL_VERSION`]。

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
/// 协议级错误（非法帧）的保留 id；正常请求 id 从 1 开始。
pub const PROTOCOL_ERROR_ID: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Hello,
    ListTools,
    CallTool,
    Health,
    Shutdown,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hello => "host/hello",
            Self::ListTools => "plugin/listTools",
            Self::CallTool => "plugin/callTool",
            Self::Health => "plugin/health",
            Self::Shutdown => "host/shutdown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "host/hello" => Some(Self::Hello),
            "plugin/listTools" => Some(Self::ListTools),
            "plugin/callTool" => Some(Self::CallTool),
            "plugin/health" => Some(Self::Health),
            "host/shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Request {
        id: u64,
        method: Method,
        params: Value,
    },
    Notification {
        method: Method,
        params: Value,
    },
    Response {
        id: u64,
        result: Value,
    },
    Error {
        id: u64,
        error: RpcError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    InvalidJson(String),
    EmptyLine,
    /// 既不是请求也不是应答（字段组合非法）。
    InvalidShape,
    UnknownMethod(String),
    MissingId,
    UnexpectedFields,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(formatter, "帧不是合法 JSON：{message}"),
            Self::EmptyLine => write!(formatter, "空行不是合法帧"),
            Self::InvalidShape => write!(formatter, "帧的字段组合非法"),
            Self::UnknownMethod(method) => write!(formatter, "未知方法：{method}"),
            Self::MissingId => write!(formatter, "请求/应答缺少 id"),
            Self::UnexpectedFields => write!(formatter, "帧带了互斥的字段"),
        }
    }
}

impl std::error::Error for FrameError {}

/// 编码一帧（不含换行；调用方负责 `\n`）。
pub fn encode(frame: &Frame) -> String {
    let value = match frame {
        Frame::Request { id, method, params } => {
            serde_json::json!({ "id": id, "method": method.as_str(), "params": params })
        }
        Frame::Notification { method, params } => {
            serde_json::json!({ "method": method.as_str(), "params": params })
        }
        Frame::Response { id, result } => serde_json::json!({ "id": id, "result": result }),
        Frame::Error { id, error } => serde_json::json!({ "id": id, "error": error }),
    };
    serde_json::to_string(&value).expect("帧可序列化")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrame {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// 解码一行（允许行尾 `\r` / `\n`）。
pub fn decode_line(line: &str) -> Result<Frame, FrameError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.trim().is_empty() {
        return Err(FrameError::EmptyLine);
    }
    let raw: RawFrame = serde_json::from_str(trimmed)
        .map_err(|error| FrameError::InvalidJson(error.to_string()))?;
    classify(raw)
}

fn classify(raw: RawFrame) -> Result<Frame, FrameError> {
    match (raw.method, raw.result, raw.error) {
        (Some(method), None, None) => {
            let parsed = Method::parse(&method).ok_or(FrameError::UnknownMethod(method))?;
            let params = raw.params.unwrap_or(Value::Null);
            match raw.id {
                Some(id) => Ok(Frame::Request {
                    id,
                    method: parsed,
                    params,
                }),
                None => Ok(Frame::Notification {
                    method: parsed,
                    params,
                }),
            }
        }
        (None, Some(result), None) => {
            let id = raw.id.ok_or(FrameError::MissingId)?;
            if raw.params.is_some() {
                return Err(FrameError::UnexpectedFields);
            }
            Ok(Frame::Response { id, result })
        }
        (None, None, Some(error)) => {
            let id = raw.id.ok_or(FrameError::MissingId)?;
            if raw.params.is_some() {
                return Err(FrameError::UnexpectedFields);
            }
            Ok(Frame::Error { id, error })
        }
        (None, None, None) => Err(FrameError::InvalidShape),
        _ => Err(FrameError::UnexpectedFields),
    }
}
