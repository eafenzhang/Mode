//! 图标面：`getApplicationIcon`。
//!
//! **M1 为部分实现**：命令已注册可调用，但一律返回 `null`。
//!
//! 理由与边界：
//! - 契约允许 `null`（`Promise<ApplicationIconInfo | null>`），且调用方显式处理了它
//!   （`cuaAppSummaryIcon.tsx` 与 `cuaListDetails.tsx` 都先判断可空再渲染），
//!   所以返回 `null` 是**契约内的取值**，不是伪造成功；
//! - 真实取值需要按平台做原生图标解码（Windows 取 exe / AUMID 的图标资源并编码成
//!   位图数据 URL，macOS 走 `NSWorkspace`），是独立于命令面注册的一件活；
//! - 用途是把 CUA 工具调用块里的 App 图标画出来（纯展示），不影响任何流程正确性。
//!
//! 因此 M1 只保证「命令存在、参数形状正确、缺值路径与旧实现一致」，
//! 原生解码列在 `docs/specs/tauri-platform-commands.md` 的 M1 未完成项里。
use protocol::{ApplicationIconInfo, ApplicationIconRequest};

/// `IPlatformService.getApplicationIcon(request: string | ApplicationIconRequest)`
///
/// TS 侧入参是 `string | ApplicationIconRequest` 的联合。**M1 只声明结构化形态**：
/// 字符串入参在适配层（`packages/ui/src/platform/tauri-invoke-shim.ts`）归一成
/// `{ locators: [{ kind: "darwin-bundle-id", value }] }` 后再过桥，
/// 因为 Tauri 的 `invoke` 参数是强类型的，联合类型无法在命令签名上表达。
#[tauri::command]
pub fn get_application_icon(_request: ApplicationIconRequest) -> Option<ApplicationIconInfo> {
    None
}
