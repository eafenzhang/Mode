/// SDK `ERROR_CODE_BY_BROKER` 的全部 17 个键（与 docs/specs/computer-use-windows-runtime.md 同步维护）。
pub const ALLOWED_CODES: [&str; 17] = [
  "permission_denied", "not_authorized", "launch_failed", "invalid_request",
  "element_unavailable", "not_settable", "not_selectable", "action_unavailable",
  "foreground_required", "controller_busy", "broker_unavailable", "version_mismatch",
  "stale_socket", "timeout", "unimplemented", "method_not_found", "internal",
];
#[derive(Debug)]
pub struct AxError { pub code: &'static str, pub message: String }
impl AxError {
  pub fn new(code: &'static str, message: impl Into<String>) -> Self {
    debug_assert!(ALLOWED_CODES.contains(&code), "non-contract code: {code}");
    Self { code, message: message.into() }
  }
  pub fn internal(m: impl Into<String>) -> Self { Self::new("internal", m) }
}
impl std::fmt::Display for AxError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.message) } }
impl std::error::Error for AxError {}
pub type AxResult<T> = Result<T, AxError>;
