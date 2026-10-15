//! 深链（路线图 M1 任务 6 的最小可用面）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` §2「事件通道」——M1 落一个示范：
//! `onOAuthCallback` 走 Tauri event + deep-link；其余订阅型方法留 M3。
//! scheme 与旧 Electron 线一致（`mode`），见 `electron-builder.config.js` 的 `protocols`。
//!
//! ## 分工
//!
//! - [`intent`]：纯解析（`mode://` / 旧 `zcode://` → 类型化意图），不碰 Tauri；
//! - [`router`]：冷启动缓存与 renderer 就绪后的回放，不碰 Tauri；
//! - 本模块：把上面两层接到 Tauri 上——注册 scheme、监听 open-url、把意图 emit 成事件。
//!
//! 事件载荷类型 [`protocol::DeepLinkEvent`] 与事件名 [`protocol::DEEP_LINK_EVENT`] 都在
//! `crates/protocol`（跨端契约的真源，由 `scripts/generate-tauri-protocol.mjs` 导出到 TS，
//! CI 断言无漂移）；渲染层侧对应
//! `packages/ui/src/platform/tauri-invoke-shim.ts` 的 `onOAuthCallback` / `onPaymentCallback`。
//!
//! ## M1 已知边界（登记不隐瞒）
//!
//! - **不做单实例转发**：Windows / Linux 上「应用已在运行 → 用户点 `mode://` 链接」会
//!   起一个新进程（新窗口）。单实例会把窗口生命周期语义变复杂，与「每个窗口一个
//!   window-scoped Local Host」（`AGENTS.md`）绑在一起，属 M3 的窗口模型决策。
//! - 冷启动（应用没在跑）路径是完整的：scheme 已注册、URL 从 argv 解析、缓存到
//!   renderer 握手后投递。
use protocol::{DeepLinkEvent, DeepLinkEventKind, DEEP_LINK_EVENT};
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub mod intent;
pub mod router;

pub use intent::{DeepLinkIntent, ACCEPTED_SCHEMES, REGISTERED_SCHEME};
pub use protocol::{DeepLinkEvent as Event, DEEP_LINK_EVENT as EVENT_NAME};
pub use router::{DeepLinkRouter, DeepLinkSink};

/// 意图 + 原文 → 事件载荷。
///
/// 放在本模块而不是 `crates/protocol`：转换需要 [`DeepLinkIntent`]，而它是本 crate 的
/// 类型；把载荷类型放在 protocol（跨端契约）与把转换放在 deeplink（行为）是两件事，
/// 硬凑到一起会让 protocol 反向依赖 deeplink。
fn event_for(raw_url: &str, intent: &DeepLinkIntent) -> DeepLinkEvent {
    let (kind, path, code) = match intent {
        DeepLinkIntent::OAuthCallback { .. } => (DeepLinkEventKind::OAuthCallback, None, None),
        DeepLinkIntent::PaymentCallback { .. } => (DeepLinkEventKind::PaymentCallback, None, None),
        DeepLinkIntent::WorkspaceOpen { path } => {
            (DeepLinkEventKind::WorkspaceOpen, Some(path.clone()), None)
        }
        DeepLinkIntent::ShareImport { code } => {
            (DeepLinkEventKind::ShareImport, None, Some(code.clone()))
        }
        DeepLinkIntent::Unrecognized { .. } => (DeepLinkEventKind::Unrecognized, None, None),
    };
    DeepLinkEvent {
        kind,
        url: raw_url.to_string(),
        path,
        code,
    }
}

/// 把意图 emit 成 Tauri 事件的投递器。
pub struct TauriSink<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriSink<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> DeepLinkSink for TauriSink<R> {
    fn deliver(&self, raw_url: &str, intent: &DeepLinkIntent) {
        let payload = event_for(raw_url, intent);
        if let Err(error) = self.app.emit(DEEP_LINK_EVENT, payload) {
            // emit 失败只可能是窗口已销毁：此时没有接收方。本 crate 不引日志插件
            // （M1 骨架刻意不扩依赖），用 stderr 兜底，不静默吞掉。
            eprintln!("[deeplink] 投递深链事件失败：{error}");
        }
    }
}

/// 深链路由器作为 Tauri 托管状态的类型别名。
pub type DeeplinkState<R> = DeepLinkRouter<TauriSink<R>>;

/// 安装深链面：注册 scheme、监听运行期回调、把冷启动 argv 里的链接排进队列。
///
/// 调用点在 `setup`（此时 `AppHandle` 已可用）。注册失败不致命（例如无权限写注册表），
/// 只记一条 stderr——**但冷启动解析仍然执行**，因为从命令行手动传入链接（开发与测试）
/// 不依赖 scheme 注册。
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    use tauri_plugin_deep_link::DeepLinkExt;

    // 注册给操作系统的 scheme 只有 `mode`（docs/specs/p2-mode-naming.md：不注册旧 scheme，
    // 避免与宿主应用争抢默认 handler）；旧 `zcode://` 只在**处理**侧兼容。
    if let Err(error) = app.deep_link().register(REGISTERED_SCHEME) {
        eprintln!("[deeplink] 注册 {REGISTERED_SCHEME}:// scheme 失败（不致命）：{error}");
    }

    // 运行期回调：macOS 的 Opened 事件，以及插件自己 emit 的 new-url。
    // 刻意不调插件的 `handle_cli_arguments`——它在 Windows/Linux 上只接受「除二进制名外
    // 恰好一个参数」，而旧线的 argv 契约（见 intent.rs 的取材说明）是整段前缀匹配、
    // 不限制其余参数个数。冷启动路径统一走本 crate 的 [`intent::extract_from_args`]。
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        let Some(router) = handle.try_state::<DeeplinkState<R>>() else {
            eprintln!("[deeplink] 路由状态尚未托管，忽略本次 open-url");
            return;
        };
        for url in event.urls() {
            router.dispatch(url.as_str());
        }
    });

    // 冷启动：链接随 argv 进来时窗口还不存在，解析结果进队列，等 notifyRendererReady 回放。
    if let Some(router) = app.try_state::<DeeplinkState<R>>() {
        if let Some(raw) = intent::extract_from_args(std::env::args()) {
            router.dispatch(&raw);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 载荷形状（camelCase、kind 线上取值、缺席而非 null）由 `crates/protocol` 的
    /// 往返测试钉住——那是跨端契约的真源。这里只钉「意图 → kind」的映射，
    /// 因为映射表在本模块。
    #[test]
    fn each_intent_maps_to_its_own_kind() {
        let cases: [(DeepLinkIntent, DeepLinkEventKind); 5] = [
            (
                DeepLinkIntent::OAuthCallback { url: String::new() },
                DeepLinkEventKind::OAuthCallback,
            ),
            (
                DeepLinkIntent::PaymentCallback { url: String::new() },
                DeepLinkEventKind::PaymentCallback,
            ),
            (
                DeepLinkIntent::WorkspaceOpen {
                    path: String::new(),
                },
                DeepLinkEventKind::WorkspaceOpen,
            ),
            (
                DeepLinkIntent::ShareImport {
                    code: String::new(),
                },
                DeepLinkEventKind::ShareImport,
            ),
            (
                DeepLinkIntent::Unrecognized { url: String::new() },
                DeepLinkEventKind::Unrecognized,
            ),
        ];
        for (intent, expected) in cases {
            assert_eq!(event_for("mode://x/y", &intent).kind, expected);
        }
    }

    #[test]
    fn event_carries_the_parsed_fields_and_the_raw_url() {
        let payload = event_for(
            "mode://workspace/open?path=C%3A%5Cwork%5Cdemo",
            &DeepLinkIntent::WorkspaceOpen {
                path: r"C:\work\demo".to_string(),
            },
        );
        assert_eq!(payload.url, "mode://workspace/open?path=C%3A%5Cwork%5Cdemo");
        assert_eq!(payload.path.as_deref(), Some(r"C:\work\demo"));
        assert_eq!(payload.code, None);

        let share = event_for(
            "mode://share/import?code=abc",
            &DeepLinkIntent::ShareImport {
                code: "abc".to_string(),
            },
        );
        assert_eq!(share.code.as_deref(), Some("abc"));
        assert_eq!(share.path, None);
    }
}
