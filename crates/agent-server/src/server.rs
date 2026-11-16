//! stdio 分帧与线上四类帧（请求 / 通知 / 应答 / 错误应答）。
//!
//! 帧类型与严格度由 `agent-protocol` 拥有（`Frame` / `RequestFrame` /
//! `NotificationFrame` / `ResponseFrame` / `ErrorResponseFrame` / `RequestId`）；
//! 本模块只负责分帧、把 `serde_json::Value` 映射成帧，以及把分派结果写回。
//!
//! 帧边界只认 `\n`：U+2028/U+2029 是 JSON 字符串里的普通字符，不能当换行；
//! CRLF 在解析前去掉行尾 `\r`；空行忽略；读到 EOF 时残留缓冲区仍按一帧处理。
//! 分帧口径与 `docs/specs/mode-protocol-client.md`「帧」一致（客户端与运行时
//! 共享同一套线上规则）。

use std::fmt;
use std::io;
use std::sync::Arc;

use agent_protocol::{ErrorObject, ErrorResponseFrame, Frame, RequestFrame, ResponseFrame};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::dispatch::AgentServer;

/// 单帧上限（防御异常刷屏）；与 `mode-protocol-client` 同口径，正常协议帧远小于此。
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// 把已解析的 JSON 值转成 `agent-protocol` 的帧；形状不合法映射为协议错误。
///
/// `agent-protocol` 的帧变体带 `deny_unknown_fields`：信封上多一个未知键会被整体
/// 拒绝。这是与 Task 1 初版本地宽松解析器的已知差异（见任务报告）。
pub fn frame_from_value(value: Value) -> Result<Frame, ServerError> {
    serde_json::from_value::<Frame>(value)
        .map_err(|error| ServerError::invalid_frame(format!("invalid frame: {error}")))
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
        let frame = frame_from_value(value)?;

        match frame {
            Frame::Request(request) => {
                let RequestFrame {
                    id, method, params, ..
                } = request;
                let response = match server.dispatch(&method, params.unwrap_or(Value::Null)) {
                    Ok(result) => Frame::Response(ResponseFrame { id, result }),
                    Err(error) => Frame::ErrorResponse(ErrorResponseFrame {
                        id,
                        error: ErrorObject {
                            code: error.code(),
                            message: error.message(),
                            data: error.data(),
                        },
                    }),
                };
                write_frame(&mut writer, &response).await?;
            }
            // 通知暂无订阅者；应答/错误应答要等反向请求（后续任务）才有 pending 可匹配。
            Frame::Notification(_) => {}
            Frame::Response(_) | Frame::ErrorResponse(_) => {}
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
