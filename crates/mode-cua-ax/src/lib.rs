#![deny(clippy::unwrap_used)]
// 模块必须 pub：集成测试（tests/*.rs）直接调用纯逻辑函数，napi 包装只做错误转码。
// 本任务只声明 apps/error；observe/capture/perform/launch/screen/uia_thread/input/
// clipboard 由 Task 3-6 创建各自文件时追加自己的 pub mod 行——提前声明会编译失败。
pub mod apps; pub mod error;
use napi_derive::napi;
use error::AxResult;

#[napi]
pub fn version() -> String { env!("CARGO_PKG_VERSION").to_string() }

// use_nullable：napi 默认把 None 字段整个省略，接口契约要求「取不到为 null」
// （含 JSON 线格式），故显式置 null。
#[napi(object, use_nullable = true)]
pub struct AppInfoNapi { pub pid: u32, pub name: Option<String>, pub bundle_id: Option<String>, pub active: bool }
#[napi(object, use_nullable = true)]
pub struct WindowRowNapi { pub window_id: u32, pub pid: u32, pub title: Option<String>,
  pub bounds: Vec<i32>, pub main: bool, pub focused: bool, pub onscreen: bool }

// napi 默认把 Rust 函数名转 camelCase（list_apps→listApps）；接口契约的导出面是
// list_apps 与 listWindows，故 list_apps 用 js_name 钉住，list_windows 靠默认转换得 listWindows。
#[napi(js_name = "list_apps")]
pub fn list_apps() -> napi::Result<Vec<AppInfoNapi>> { to_napi(apps::list_apps()) }

#[napi]
pub fn list_windows(pid: Option<u32>) -> napi::Result<Vec<WindowRowNapi>> { to_napi(apps::list_windows(pid)) }

fn to_napi<T>(r: AxResult<T>) -> napi::Result<T> {
  // napi 2.16 无 Status::GenericError 变体，语义等价的是 GenericFailure（编译器强制适配）。
  r.map_err(|e| napi::Error::new(napi::Status::GenericFailure,
    format!("{}:{}", e.code, e.message)))
}
