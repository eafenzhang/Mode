#![deny(clippy::unwrap_used)]
// 模块必须 pub：集成测试（tests/*.rs）直接调用纯逻辑函数，napi 包装只做错误转码。
// capture/perform/input/clipboard 已落地（Task 4/5）；launch/screen 由 Task 6
// 创建各自文件时追加自己的 pub mod 行——提前声明会编译失败。
pub mod apps; pub mod capture; pub mod clipboard; pub mod error; pub mod input; pub mod observe;
pub mod perform; pub mod uia_thread;
use napi_derive::napi;
use error::AxResult;

// 全部 napi 导出统一 js_name 钉 snake_case（含 version）。
#[napi(js_name = "version")]
pub fn version() -> String { env!("CARGO_PKG_VERSION").to_string() }

// use_nullable：napi 默认把 None 字段整个省略，接口契约要求「取不到为 null」
// （含 JSON 线格式），故显式置 null。
#[napi(object, use_nullable = true)]
pub struct AppInfoNapi { pub pid: u32, pub name: Option<String>, pub bundle_id: Option<String>, pub active: bool }
#[napi(object, use_nullable = true)]
pub struct WindowRowNapi { pub window_id: u32, pub pid: u32, pub title: Option<String>,
  pub bounds: Vec<i32>, pub main: bool, pub focused: bool, pub onscreen: bool }

// 请求结构是入参面：maxElements 缺省走 3000（plan 的 broker 面为 {windowId, maxElements?}），
// 不加 use_nullable（它面向出参的「取不到为 null」），Option 让 TS 侧呈现为可选字段。
#[napi(object)]
pub struct ObserveRequestNapi { pub window_id: u32, pub max_elements: Option<u32> }

// use_nullable：None 字段显式落 null 而不是被 JSON 省略（同 Task 2 口径）。
// serde 供 examples/probe 打印 observe 全树 JSON，rename_all 对齐 napi 运行时的 camelCase 键。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[napi(object, use_nullable = true)]
pub struct ObserveResultNapi { pub window_title: Option<String>, pub focused_index: Option<u32>,
  pub enumeration_complete: bool, pub elements: Vec<ElementNapi> }
#[derive(serde::Serialize)]
#[napi(object, use_nullable = true)]
pub struct ElementNapi { pub index: u32, pub kind: String, pub title: Option<String>,
  pub value: Option<String>, pub bounds: Vec<i32>, pub actions: Vec<String>,
  pub enabled: bool, pub offscreen: bool }

// napi 默认把 Rust 函数名转 camelCase（list_apps→listApps、list_windows→listWindows），
// 与 Task 8 后端映射表冲突（broker method 与 addon 函数同名 1:1 直调）。导出面统一钉为
// snake_case：本任务 list_apps/list_windows 均用 js_name，后续 observe/capture/perform/
// launch_app/screen_probe 同样加 js_name 钉住。
#[napi(js_name = "list_apps")]
pub fn list_apps() -> napi::Result<Vec<AppInfoNapi>> { to_napi(apps::list_apps()) }

#[napi(js_name = "list_windows")]
pub fn list_windows(pid: Option<u32>) -> napi::Result<Vec<WindowRowNapi>> { to_napi(apps::list_windows(pid)) }

#[napi(js_name = "observe")]
pub fn observe(req: ObserveRequestNapi) -> napi::Result<ObserveResultNapi> {
  to_napi(observe::observe(req.window_id,
    req.max_elements.unwrap_or(observe::DEFAULT_MAX_ELEMENTS)))
}

// payload 是入参面的自由 JSON（形状随 kind 变），serde_json::Value 经 napi serde 特性互转；
// 不加 use_nullable（入参口径同 ObserveRequestNapi）。
#[napi(object)]
pub struct PerformRequestNapi {
  pub kind: String,
  pub window_id: u32,
  pub payload: serde_json::Value,
}

// use_nullable：结果面口径（Task 2/4），字段恒在时为无害一致。
#[napi(object, use_nullable = true)]
pub struct PerformResultNapi {
  /// "dispatched" | "not_dispatched" | "unknown"
  pub dispatched: String,
}

#[napi(js_name = "perform")]
pub fn perform_napi(req: PerformRequestNapi) -> napi::Result<PerformResultNapi> {
  to_napi(
    perform::parse_req(&req.kind, &req.payload)
      .and_then(|parsed| perform::perform(&parsed, req.window_id))
      .map(|dispatched| PerformResultNapi { dispatched }),
  )
}

fn to_napi<T>(r: AxResult<T>) -> napi::Result<T> {
  // napi 2.16 无 Status::GenericError 变体，语义等价的是 GenericFailure（编译器强制适配）。
  r.map_err(|e| napi::Error::new(napi::Status::GenericFailure,
    format!("{}:{}", e.code, e.message)))
}
