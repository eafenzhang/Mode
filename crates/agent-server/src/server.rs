//! stdio 分帧与线上四类帧（请求 / 通知 / 应答 / 错误应答）。
//!
//! 帧边界只认 `\n`：U+2028/U+2029 是 JSON 字符串里的普通字符，不能当换行；
//! CRLF 在解析前去掉行尾 `\r`；空行忽略；读到 EOF 时残留缓冲区仍按一帧处理。
//! 分帧口径与 `docs/specs/mode-protocol-client.md`「帧」一致（客户端与运行时
//! 共享同一套线上规则）。

use std::fmt;
use std::io;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::dispatch::AgentServer;

/// 单帧上限（防御异常刷屏）；与 `mode-protocol-client` 同口径，正常协议帧远小于此。
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// 请求 id：线上数字（host 发请求）与字符串（运行时发反向请求）都出现过。
///
/// 应答与请求靠 id 关联时不能假定是数字；两端共用同一个 id 空间。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl RequestId {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Number),
            Value::String(text) => Some(Self::String(text.clone())),
            _ => None,
        }
    }
}

/// 一条线上帧。四个变体覆盖 Mode Protocol 的全部帧形状：
///
/// | 线上形状 | 变体 |
/// | --- | --- |
/// | `{id, method, params}` | [`Frame::Request`] |
/// | `{method, params}` | [`Frame::Notification`] |
/// | `{id, result}` | [`Frame::Response`] |
/// | `{id, error:{code, message, data?}}` | [`Frame::Error`] |
///
/// 解析时 `params` 缺省为 `Value::Null`；序列化时请求/通知总是写出 `params`。
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: RequestId,
        result: Value,
    },
    Error {
        id: RequestId,
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

impl Frame {
    /// 从已解析的 JSON 值构造帧；形状不合法时返回 [`ServerError::InvalidFrame`]。
    pub fn from_value(value: Value) -> Result<Self, ServerError> {
        let Some(object) = value.as_object() else {
            return Err(ServerError::invalid_frame("frame is not a JSON object"));
        };
        let id = match object.get("id") {
            Some(value) => Some(RequestId::from_value(value).ok_or_else(|| {
                ServerError::invalid_frame("frame id must be a number or string")
            })?),
            None => None,
        };
        let method = match object.get("method") {
            Some(Value::String(method)) => Some(method.clone()),
            Some(_) => return Err(ServerError::invalid_frame("frame method must be a string")),
            None => None,
        };

        match (id, method) {
            (Some(id), Some(method)) => Ok(Self::Request {
                id,
                method,
                params: frame_params(object),
            }),
            (None, Some(method)) => Ok(Self::Notification {
                method,
                params: frame_params(object),
            }),
            (Some(id), None) => {
                if let Some(result) = object.get("result") {
                    return Ok(Self::Response {
                        id,
                        result: result.clone(),
                    });
                }
                let Some(error) = object.get("error") else {
                    return Err(ServerError::invalid_frame(
                        "frame with id has neither result, error nor method",
                    ));
                };
                let error = error
                    .as_object()
                    .ok_or_else(|| ServerError::invalid_frame("frame error must be an object"))?;
                let code = error.get("code").and_then(Value::as_i64).ok_or_else(|| {
                    ServerError::invalid_frame("frame error.code must be an integer")
                })?;
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ServerError::invalid_frame("frame error.message must be a string")
                    })?
                    .to_string();
                Ok(Self::Error {
                    id,
                    code,
                    message,
                    data: error.get("data").cloned(),
                })
            }
            (None, None) => Err(ServerError::invalid_frame(
                "frame has neither id nor method",
            )),
        }
    }

    /// 转成线上 JSON。错误应答写成 `{id, error:{code, message, data?}}`。
    pub fn to_value(&self) -> Value {
        match self {
            Self::Request { id, method, params } => {
                serde_json::json!({ "id": id, "method": method, "params": params })
            }
            Self::Notification { method, params } => {
                serde_json::json!({ "method": method, "params": params })
            }
            Self::Response { id, result } => serde_json::json!({ "id": id, "result": result }),
            Self::Error {
                id,
                code,
                message,
                data,
            } => {
                let mut error = Map::new();
                error.insert("code".to_string(), Value::from(*code));
                error.insert("message".to_string(), Value::String(message.clone()));
                if let Some(data) = data {
                    error.insert("data".to_string(), data.clone());
                }
                serde_json::json!({ "id": id, "error": error })
            }
        }
    }
}

fn frame_params(object: &Map<String, Value>) -> Value {
    object.get("params").cloned().unwrap_or(Value::Null)
}

impl Serialize for Frame {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Frame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Frame::from_value(value).map_err(serde::de::Error::custom)
    }
}

/// Agent Server 的失败面。`code` 是 JSON-RPC 错误码，直接写进错误应答。
#[derive(Debug)]
pub enum ServerError {
    /// `-32601`：未知/未实现的方法（C1 未支持的方法显式走这里，不返回空结果）。
    MethodNotFound { method: String },
    /// `-32602`：参数不合法（后续方法 handler 使用）。
    InvalidParams { message: String },
    /// `-32700`：非法 JSON 或帧形状不合法，按协议错误关闭连接。
    InvalidFrame { message: String },
    /// `-32603`：内部错误。
    Internal { message: String },
    /// stdio 传输错误。
    Io(io::Error),
}

impl ServerError {
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self::MethodNotFound {
            method: method.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::InvalidParams {
            message: message.into(),
        }
    }

    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn code(&self) -> i64 {
        match self {
            Self::MethodNotFound { .. } => -32601,
            Self::InvalidParams { .. } => -32602,
            Self::InvalidFrame { .. } => -32700,
            Self::Internal { .. } | Self::Io(_) => -32603,
        }
    }

    /// 写进线上错误应答的 `message`。未知方法固定 `Method not found`（spec §5.1）。
    pub fn message(&self) -> String {
        match self {
            Self::MethodNotFound { .. } => "Method not found".to_string(),
            Self::InvalidParams { message }
            | Self::InvalidFrame { message }
            | Self::Internal { message } => message.clone(),
            Self::Io(error) => format!("stdio transport error: {error}"),
        }
    }

    /// 错误应答的可选 `data`。当前不需要附加结构化细节。
    pub fn data(&self) -> Option<Value> {
        None
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MethodNotFound { method } => {
                write!(formatter, "Method not found: {method}")
            }
            Self::InvalidParams { message }
            | Self::InvalidFrame { message }
            | Self::Internal { message } => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "stdio transport error: {error}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 在任意异步双向流上跑 `app-server --stdio` 循环。
///
/// 逐行读取（只认 `\n`）→ 解析 JSON → 分派 → 写回复；EOF 干净返回 `Ok(())`。
/// 请求分派失败时写成错误应答继续服务；非法 JSON / 非法帧返回错误并关闭连接。
pub async fn run_stdio<R, W>(
    reader: R,
    mut writer: W,
    server: Arc<AgentServer>,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .await
            .map_err(ServerError::Io)?;
        if read == 0 {
            // EOF：`read_until` 已先把残留缓冲区交回（没有换行也会返回一帧），这里退出。
            return Ok(());
        }
        if line.len() > MAX_FRAME_BYTES {
            return Err(ServerError::invalid_frame(format!(
                "frame exceeds {MAX_FRAME_BYTES} bytes"
            )));
        }
        // 只认 `\n` 为边界；CRLF 在解析前去掉行尾 `\r`。U+2028/U+2029 是多字节
        // UTF-8 字符，不会命中上面的 `\n`。
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|error| ServerError::invalid_frame(format!("invalid JSON frame: {error}")))?;
        let frame = Frame::from_value(value)?;

        match frame {
            Frame::Request { id, method, params } => {
                let response = match server.dispatch(&method, params) {
                    Ok(result) => Frame::Response { id, result },
                    Err(error) => Frame::Error {
                        id,
                        code: error.code(),
                        message: error.message(),
                        data: error.data(),
                    },
                };
                write_frame(&mut writer, &response).await?;
            }
            // 通知暂无订阅者；应答/错误应答要等反向请求（后续任务）才有 pending 可匹配。
            Frame::Notification { .. } => {}
            Frame::Response { .. } | Frame::Error { .. } => {}
        }
    }
}

async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<(), ServerError>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| ServerError::internal(format!("serialize frame failed: {error}")))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(ServerError::Io)?;
    writer.flush().await.map_err(ServerError::Io)?;
    Ok(())
}
