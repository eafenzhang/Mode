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
//! ## 三条入口，一条投递路径
//!
//! 深链到达本进程只有三种方式，**它们共用 [`dispatch_raw`] 这一条投递实现**（`DeepLinkRouter`
//! 的缓存/回放时序 + `TauriSink` 的 `emit`），所以「链接怎么进来的」不影响「链接怎么到渲染层」：
//!
//! | 入口 | 何时 | 谁接线 |
//! | --- | --- | --- |
//! | 运行期 `open-url`（macOS）/ 插件 emit 的 `new-url` | 应用已在运行、操作系统把链接交给它 | [`install`] |
//! | 冷启动 argv | 应用没在跑，链接随命令行进来 | [`install`] → 等 renderer 握手后回放 |
//! | **第二实例的 argv**（M3 Task 4 · DR-T4-1） | 应用已在运行、用户又点了一次 `mode://` 链接，操作系统**起了第二个进程** | [`forward_second_instance_args`]（壳的单实例回调调它） |
//!
//! 第三条的存在理由：Windows / Linux 上点击链接一定会起新进程，而第二个进程若走完整启动路径
//! 就会**再建一个 Host、再建一个窗口**（第二份 workspace 状态）。单实例插件在**建窗口之前**
//! 截住它、把 argv 交回第一个进程，**转发的落地就是上面这条既有投递路径**——不新增 IPC、
//! 不新增事件名。窗口模型上的代价见 `docs/specs/tauri-platform-commands.md` 的单实例一节。
//!
//! 冷启动（应用没在跑）路径是完整的：scheme 已注册、URL 从 argv 解析、缓存到
//! renderer 握手后投递。
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

/// 托管深链路由状态（幂等）。
///
/// **只托管状态，不注册 scheme**：注册会把 `mode://` 的默认 handler 指向当前可执行文件
/// （见 [`install`]），那是真机上的持久副作用。产品壳走 `install`（两者都要），
/// 而只做转发接线的测试 harness 用本函数——它要能**接住**深链，但不该改用户的注册表。
pub fn manage_state<R: Runtime>(app: &AppHandle<R>) {
    if app.try_state::<DeeplinkState<R>>().is_none() {
        app.manage(DeepLinkRouter::new(TauriSink::new(app.clone())));
    }
}

/// 把一条原始深链投进路由（**三条入口唯一的投递路径**）。
///
/// 返回 `false` 表示这次投递没有落地：路由状态还没托管（`setup` 尚未跑到），或者这条串
/// 根本不是本应用认的深链（scheme 不认识）。两种情况都**只留痕不抛**——深链是尽力而为的入口，
/// 让一次未识别的链接掀翻启动路径是本末倒置。
pub fn dispatch_raw<R: Runtime>(app: &AppHandle<R>, raw: &str) -> bool {
    let Some(router) = app.try_state::<DeeplinkState<R>>() else {
        eprintln!("[deeplink] 路由状态尚未托管，忽略本次深链：{raw}");
        return false;
    };
    router.dispatch(raw).is_some()
}

/// 第二实例的 argv 里要转交给已运行实例的那条深链（[`intent::extract_from_args`] 的语义面）。
///
/// 单独有这个函数是因为**它才是单实例转发的判据**：`None` = 这次启动没有链接
/// （用户又点了一次图标），此时壳只聚焦既有窗口、不转任何东西。抽成纯函数让「转发什么」
/// 可以在没有 `AppHandle` 的情况下被断言（见 `tests/intent.rs`）。
pub fn forwardable_second_instance_url<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    intent::extract_from_args(args)
}

/// 第二实例转发（DR-T4-1）：把转发进来的 argv 解析成深链，走**既有**投递路径送进渲染层。
///
/// 返回值是**本次真正转发出去**的链接原文（`None` = 没有可转发的东西，或投递没落地），
/// 供壳记一条诊断。**本函数不建窗口、不建 Host、不碰进程**——它只投一条事件。
pub fn forward_second_instance_args<R: Runtime>(
    app: &AppHandle<R>,
    args: &[String],
) -> Option<String> {
    let raw = forwardable_second_instance_url(args)?;
    dispatch_raw(app, &raw).then_some(raw)
}

/// 安装深链面：托管路由状态、注册 scheme、监听运行期回调、把冷启动 argv 里的链接排进队列。
///
/// 调用点在 `setup`（此时 `AppHandle` 已可用）。注册失败不致命（例如无权限写注册表），
/// 只记一条 stderr——**但冷启动解析仍然执行**，因为从命令行手动传入链接（开发与测试）
/// 不依赖 scheme 注册。
///
/// **注册 scheme 是持久副作用**（它把 `mode://` 的默认 handler 指向当前可执行文件）：
/// 只做接线的测试 harness 应当用 [`manage_state`]，不要走本函数。
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    use tauri_plugin_deep_link::DeepLinkExt;

    // 先托管状态：下面两条路（运行期回调、冷启动 argv）都要用它。幂等，所以调用方
    // 之前 manage 过也不会重复。
    manage_state(app);

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
        for url in event.urls() {
            // 与第二实例转发共用同一条投递路径（见 [`dispatch_raw`]）。
            dispatch_raw(&handle, url.as_str());
        }
    });

    // 冷启动：链接随 argv 进来时窗口还不存在，解析结果进队列，等 notifyRendererReady 回放。
    // 这里刻意不走 `forwardable_second_instance_url`：本进程**不是**第二实例（单实例插件
    // 已经把第二实例截在建窗口之前），而两条路径的判据同源（同一个 `extract_from_args`）。
    if let Some(raw) = intent::extract_from_args(std::env::args()) {
        dispatch_raw(app, &raw);
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
