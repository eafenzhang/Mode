//! CUA raw input 注入端口：一次 broker `raw_input` 调用与错误码收敛。
//!
//! 真源：`packages/services/src/cua-permission-broker/cuaRawInput.ts` 的
//! `createCuaRawInputInjector`。本层不重试、不做前台/权限校验，也不持有 helper 生命周期。

use std::fmt;
use std::sync::Arc;

use serde_json::Value;

pub const DEFAULT_RAW_INPUT_TIMEOUT_MS: u64 = 5_000;

const UNREACHABLE_BROKER_CODES: [&str; 4] = [
    "stale_socket",
    "timeout",
    "unavailable",
    "broker_unavailable",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuaHelperError {
    pub code: String,
    pub message: String,
}

impl CuaHelperError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CuaHelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CuaHelperError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerCallError {
    Helper(CuaHelperError),
    Broker { code: String, message: String },
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawInputBrokerResult {
    pub dispatched: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawInputDispatch {
    pub dispatched: f64,
}

pub type SocketPathResolver =
    Arc<dyn Fn() -> Result<Option<String>, BrokerCallError> + Send + Sync>;
pub type BrokerCall = Arc<
    dyn Fn(String, Vec<Value>, u64) -> Result<RawInputBrokerResult, BrokerCallError> + Send + Sync,
>;

pub struct RawInputInjectorOptions {
    pub resolve_socket_path: SocketPathResolver,
    pub call_broker: BrokerCall,
    pub timeout_ms: u64,
}

pub struct RawInputInjector {
    options: RawInputInjectorOptions,
}

impl RawInputInjector {
    pub fn new(options: RawInputInjectorOptions) -> Self {
        Self { options }
    }

    /// 只拦数组门；逐条事件形状归 helper 与 Rust `parse_raw_input` 校验。
    pub fn inject(&self, events: Value) -> Result<RawInputDispatch, CuaHelperError> {
        let Value::Array(events) = events else {
            return Err(CuaHelperError::new(
                "invalid_request",
                "raw_input events must be an array",
            ));
        };
        let socket_path = (self.options.resolve_socket_path)().map_err(map_broker_error)?;
        let Some(socket_path) = socket_path else {
            return Err(CuaHelperError::new(
                "helper_unavailable",
                "Computer Use Helper is not reachable for raw input",
            ));
        };
        let result = (self.options.call_broker)(socket_path, events, self.options.timeout_ms)
            .map_err(map_broker_error)?;
        let Some(dispatched) = result.dispatched else {
            return Err(CuaHelperError::new(
                "helper_unavailable",
                "Computer Use Helper returned an invalid raw_input result",
            ));
        };
        Ok(RawInputDispatch { dispatched })
    }
}

fn map_broker_error(error: BrokerCallError) -> CuaHelperError {
    match error {
        BrokerCallError::Helper(error) => error,
        BrokerCallError::Broker { code, message } => {
            let code = if UNREACHABLE_BROKER_CODES.contains(&code.as_str()) {
                "helper_unavailable".to_string()
            } else {
                code
            };
            CuaHelperError { code, message }
        }
        BrokerCallError::Other(message) => CuaHelperError::new("helper_unavailable", message),
    }
}
