//! 图标面：`getApplicationIcon`。
//!
//! 规格：`docs/specs/tauri-platform-commands.md`。契约是
//! `Promise<ApplicationIconInfo | null>`，**取不到就返回 `null`、绝不抛**
//! （契约允许 `null` 且调用方已显式处理；`cuaAppSummaryIcon.tsx` / `cuaListDetails.tsx`
//! 都先判断可空再渲染，所以 `null` 是**契约内的取值**，不是伪造成功）。
//!
//! ## 实现口径（M3 Task 4 起不再是占位）
//!
//! - **Windows**：`crate::icon_windows` 做原生解码（Shell 取图标资源 + GDI 取像素 + PNG/base64），
//!   按 locator 顺序尝试，全部失败返回 `null`；
//! - **macOS 半边未实现且本阶段不验**（Ruling 20：本阶段没有 macOS 构建，写了也验不了）——
//!   非 Windows 平台上本命令**仍然返回 `null`**，这是契约内的取值而不是错误。
//!   该登记写在 spec 的未完成项里，不沉默省略。
//!
//! 用途是把 CUA 工具调用块里的 App 图标画出来（纯展示），不影响任何流程正确性——
//! 所以「取不到」用 `null` 表达就够了，不需要为它引入错误通道。
use protocol::{ApplicationIconInfo, ApplicationIconRequest};

/// `IPlatformService.getApplicationIcon(request: string | ApplicationIconRequest)`
///
/// TS 侧入参是 `string | ApplicationIconRequest` 的联合。**本命令只声明结构化形态**：
/// 字符串入参在适配层（`packages/ui/src/platform/tauri-invoke-shim.ts` 的
/// `normalizeApplicationIconRequest`，注入层 `crates/sidecar-host/src/window_mode.ts` 同款）
/// 归一成 `{ locators: [{ kind: …, value }] }` 后再过桥，因为 Tauri 的 `invoke` 参数是强类型的，
/// 联合类型无法在命令签名上表达。两处口径的一致性由 `windowModeInjection.test.mts` 对拍。
#[tauri::command(rename = "getApplicationIcon")]
pub fn get_application_icon(request: ApplicationIconRequest) -> Option<ApplicationIconInfo> {
    icon_data_url(&request.locators).map(|icon_data_url| ApplicationIconInfo { icon_data_url })
}

/// 平台门面：Windows 走原生解码，其余平台一律 `null`（含 macOS 半边未实现的口径）。
#[cfg(target_os = "windows")]
fn icon_data_url(locators: &[protocol::ApplicationIconLocator]) -> Option<String> {
    crate::icon_windows::icon_data_url(locators)
}

#[cfg(not(target_os = "windows"))]
fn icon_data_url(_locators: &[protocol::ApplicationIconLocator]) -> Option<String> {
    // macOS 半边未实现（Ruling 20）；Linux 同理不在本阶段范围。返回契约内的 `null`。
    None
}
