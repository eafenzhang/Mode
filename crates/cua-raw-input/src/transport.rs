//! CUA broker socket 传输：单请求单连接、行分隔 JSON、超时与健康探测。
//!
//! 真源：`packages/mode-cua/broker.js` 的 `callBrokerMethod` / `probeHelperHealth`。
//! 装帧、方法表与路径铸造在 [`crate::broker_protocol`]；本模块只做 IO 与错误收敛。
//!
//! 平台：POSIX 用 Unix domain socket；Windows 用命名管道。两者都走 tokio，
//! 超时用 `tokio::time::timeout`，丢弃 future 后关闭连接。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::time::Instant;

use crate::injector::{BrokerCall, BrokerCallError, RawInputBrokerResult};

/// 单个响应行的上限；TS 无上限，这里防止对端异常时无界缓冲。
pub const MAX_RESPONSE_LINE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub const DEFAULT_HEALTH_TRY_TIMEOUT: Duration = Duration::from_millis(500);

/// TS `BrokerError`：`code` 是稳定分流依据，`message` 原样上抛。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerError {
    pub code: String,
    pub message: String,
}

impl BrokerError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn timeout() -> Self {
        Self::new("timeout", "broker call timed out")
    }

    fn closed() -> Self {
        Self::new("stale_socket", "connection closed")
    }
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for BrokerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperHealth {
    pub bundle_id: Option<String>,
    pub pid: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct HealthProbeOptions {
    pub timeout: Duration,
    pub poll_interval: Duration,
    pub per_try_timeout: Duration,
}

impl Default for HealthProbeOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_HEALTH_TIMEOUT,
            poll_interval: DEFAULT_HEALTH_POLL_INTERVAL,
            per_try_timeout: DEFAULT_HEALTH_TRY_TIMEOUT,
        }
    }
}

pub struct BrokerClient {
    socket_path: String,
}

impl BrokerClient {
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// `callBrokerMethod`：单请求单连接；成功返回 `result`，其余收敛为 `BrokerError`。
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, BrokerError> {
        let request = serde_json::to_vec(&json!({
            "id": "1",
            "method": method,
            "params": params,
        }))
        .map_err(|error| BrokerError::new("internal", error.to_string()))?;

        let exchange = async {
            let stream = match connect(&self.socket_path).await {
                Ok(stream) => stream,
                Err(error) => return Err(BrokerError::new("stale_socket", error.to_string())),
            };
            let mut writer = stream;
            if let Err(error) = writer.write_all(&request).await {
                return Err(BrokerError::new("stale_socket", error.to_string()));
            }
            if let Err(error) = writer.write_all(b"\n").await {
                return Err(BrokerError::new("stale_socket", error.to_string()));
            }
            if let Err(error) = writer.flush().await {
                return Err(BrokerError::new("stale_socket", error.to_string()));
            }

            let mut line = String::new();
            let read = {
                let reader = BufReader::new(writer);
                let mut limited = reader.take((MAX_RESPONSE_LINE_BYTES + 1) as u64);
                limited.read_line(&mut line).await
            };
            match read {
                Ok(0) => Err(BrokerError::closed()),
                Ok(_) => {
                    if line.len() > MAX_RESPONSE_LINE_BYTES {
                        return Err(BrokerError::new(
                            "internal",
                            "invalid broker response: line too long",
                        ));
                    }
                    parse_response(&line)
                }
                Err(error) => Err(BrokerError::new("stale_socket", error.to_string())),
            }
        };

        match tokio::time::timeout(timeout, exchange).await {
            Ok(result) => result,
            Err(_) => Err(BrokerError::timeout()),
        }
    }

    /// `probeHelperHealth`：deadline 内轮询 `health`，只接受 `pid = null | number`。
    pub async fn probe_health(
        &self,
        options: HealthProbeOptions,
    ) -> Result<HelperHealth, BrokerError> {
        let deadline = Instant::now() + options.timeout;
        let mut last_error: Option<BrokerError> = None;
        loop {
            if Instant::now() >= deadline {
                break;
            }
            match self
                .call("health", json!({}), options.per_try_timeout)
                .await
            {
                Ok(result) => match parse_health(&result) {
                    Some(health) => return Ok(health),
                    None => {
                        last_error = Some(BrokerError::new("internal", "health payload invalid"));
                    }
                },
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(options.poll_interval).await;
        }
        let message = last_error
            .map(|error| error.message)
            .unwrap_or_else(|| "unknown".to_string());
        Err(BrokerError::new(
            "broker_unavailable",
            format!("helper health probe failed: {message}"),
        ))
    }
}

/// 把异步 [`BrokerClient`] 包成注入端口使用的同步 `BrokerCall`。
///
/// `runtime` 必须是调用方持有的运行时句柄；本函数会在调用线程上 `block_on`，
/// 因此**不能**从该运行时的 worker 线程调用（宿主应把它用于同步桥/阻塞线程）。
pub fn broker_call(runtime: tokio::runtime::Handle) -> BrokerCall {
    Arc::new(move |socket_path, events, timeout_ms| {
        let client = BrokerClient::new(socket_path);
        let params = serde_json::json!({ "events": events });
        match runtime.block_on(client.call("raw_input", params, Duration::from_millis(timeout_ms))) {
            Ok(value) => Ok(RawInputBrokerResult {
                dispatched: value.get("dispatched").and_then(Value::as_f64),
            }),
            Err(error) => Err(BrokerCallError::Broker {
                code: error.code,
                message: error.message,
            }),
        }
    })
}

fn parse_response(line: &str) -> Result<Value, BrokerError> {
    let value: Value = serde_json::from_str(line.trim_end_matches(['\r', '\n']))
        .map_err(|error| {
            BrokerError::new("internal", format!("invalid broker response: {error}"))
        })?;
    let Some(object) = value.as_object() else {
        return Err(BrokerError::new(
            "internal",
            "invalid broker response: not an object",
        ));
    };
    if object.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(object.get("result").cloned().unwrap_or(Value::Null));
    }
    let error = object.get("error").and_then(Value::as_object);
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("broker error");
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("internal");
    Err(BrokerError::new(code, message))
}

fn parse_health(result: &Value) -> Option<HelperHealth> {
    let object = result.as_object()?;
    // TS 要求 pid 字段存在且为 null 或 number；缺失视为无效。
    let pid = match object.get("pid")? {
        Value::Null => None,
        value => Some(value.as_i64()?),
    };
    // bundleId 非字符串一律按 null（与 TS `typeof ... === "string" ? ... : null` 一致）。
    let bundle_id = object
        .get("bundleId")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(HelperHealth { bundle_id, pid })
}

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

#[cfg(unix)]
async fn connect(path: &str) -> io::Result<Box<dyn AsyncStream>> {
    Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
}

#[cfg(windows)]
async fn connect(path: &str) -> io::Result<Box<dyn AsyncStream>> {
    Ok(Box::new(
        tokio::net::windows::named_pipe::ClientOptions::new().open(path)?,
    ))
}
