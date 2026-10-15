//! `mode://` 深链的解析面（路线图 M1 任务 6 的最小面）。
//!
//! ## 取材口径
//!
//! 本模块按 `docs/specs/clean-room-policy.md` §黑盒对拍编写：行为规格取自
//! `packages/desktop/tests/deep-link-scheme-compat.test.mjs` 的断言（输入 → 输出），
//! 未阅读旧实现源码。断言语义逐条对应到本文件的用例：
//!
//! | 旧线测试断言 | 本模块对应 |
//! | --- | --- |
//! | `isOAuthCallbackUrl(mode://oauth/callback)` = true | [`parse`] → [`DeepLinkIntent::OAuthCallback`] |
//! | `isOAuthCallbackUrl(zcode://oauth/callback)` = true | 同上（改名前的 scheme 仍被处理） |
//! | `isOAuthCallbackUrl(https://oauth/callback)` = false | [`parse`] → `None` |
//! | `zcode://workspace/open?path=...` → path | [`DeepLinkIntent::WorkspaceOpen`] |
//! | `zcode://share/import?code=...` → code | [`DeepLinkIntent::ShareImport`] |
//! | argv 里整段前缀匹配、词中间的 URL 前缀不算 | [`extract_from_args`] |
//!
//! ## 注册与处理是两件事
//!
//! 注册给操作系统的 scheme **只有** `mode`（`docs/specs/p2-mode-naming.md`：不注册旧
//! scheme，避免与宿主应用争抢默认 handler）；但**处理**两种 scheme 的传入链接，
//! 这样改名前的旧链接仍然可用。见 [`ACCEPTED_SCHEMES`] 与 [`REGISTERED_SCHEME`]。

/// 实际注册给操作系统的 scheme。只此一个。
pub const REGISTERED_SCHEME: &str = "mode";

/// 会被解析处理的 scheme：注册的那个 + 改名前的旧 scheme。
pub const ACCEPTED_SCHEMES: &[&str] = &["mode", "zcode"];

/// 一条深链解析出的意图。
///
/// 未识别的深链也保留为 [`DeepLinkIntent::Unrecognized`] 而不是丢掉：
/// 调用方需要能观察到「收到了一条深链但没人认领」，否则路由问题会静默消失。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkIntent {
    /// `mode://oauth/callback?state=...` —— OAuth 授权回调。
    OAuthCallback { url: String },
    /// `mode://payment/callback?...` —— 支付回调。
    PaymentCallback { url: String },
    /// `mode://workspace/open?path=...` —— 打开指定工作区。
    WorkspaceOpen { path: String },
    /// `mode://share/import?code=...` —— 会话分享导入。
    ShareImport { code: String },
    /// scheme 认识，但**没有一条规则能处理它**：host/path 不在路由表里，
    /// 或该路由的必需查询参数缺失（如 `workspace/open` 没有 `path`）。
    ///
    /// 不拆成两个变体是刻意的：两者对调用方是同一个处置（都不路由、都要能观察到），
    /// 而区分它们所需的信息事件载荷里的 `url` 原文已经带着。
    Unrecognized { url: String },
}

impl DeepLinkIntent {
    /// 事件载荷用的 URL 原文（`Unrecognized` 也带得出来）。
    pub fn raw_url(&self) -> &str {
        match self {
            Self::OAuthCallback { url }
            | Self::PaymentCallback { url }
            | Self::Unrecognized { url } => url,
            // WorkspaceOpen / ShareImport 只保留解析后的字段，没有原文；
            // 需要原文的调用方从 `extract_from_args` 的返回值自己留一份。
            Self::WorkspaceOpen { .. } | Self::ShareImport { .. } => "",
        }
    }
}

/// 解析一条深链。
///
/// 返回 `None` 表示**不是**本应用接受的深链（scheme 不认识、或不是 `//host/path` 形态）。
/// 这与 `Unrecognized` 的区别是「该不该理」与「理了但没规则」。
pub fn parse(raw: &str) -> Option<DeepLinkIntent> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    if !ACCEPTED_SCHEMES.contains(&url.scheme()) {
        return None;
    }

    // 深链必须是 `scheme://host/...`：没有 host 的 `mode:oauth/callback` 形态
    // 在旧线上不存在，这里也不认，避免把两种写法当成同一条路由。
    let host = url.host_str()?;
    let path = url.path();
    let url_string = url.as_str().to_string();

    match (host, path) {
        ("oauth", "/callback") => Some(DeepLinkIntent::OAuthCallback { url: url_string }),
        ("payment", "/callback") => Some(DeepLinkIntent::PaymentCallback { url: url_string }),
        // 路由命中但必需参数缺失时**不能**用 `?` 往下漏：那会让 `parse` 返回 `None`，
        // 调用方读到的是「这不是本应用的深链」，而事实是「是，但我们处理不了」。
        ("workspace", "/open") => Some(match query_value(&url, "path") {
            Some(path) => DeepLinkIntent::WorkspaceOpen { path },
            None => DeepLinkIntent::Unrecognized { url: url_string },
        }),
        ("share", "/import") => Some(match query_value(&url, "code") {
            Some(code) => DeepLinkIntent::ShareImport { code },
            None => DeepLinkIntent::Unrecognized { url: url_string },
        }),
        _ => Some(DeepLinkIntent::Unrecognized { url: url_string }),
    }
}

/// 取查询参数并按百分号解码。缺失或空串都当「没给」，由调用方按未识别处理。
fn query_value(url: &url::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

/// 从命令行参数里取出深链（Windows / Linux 的 deep link 走 argv 传入）。
///
/// 判据是**整段前缀**匹配 `scheme://`，不是「串里出现 scheme」：
/// `C:\work\asmode.txt` 这类路径不能因为含 `mode` 就被当成链接
/// （旧线测试有这条断言）。
pub fn extract_from_args<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().find_map(|arg| {
        let arg = arg.as_ref();
        ACCEPTED_SCHEMES
            .iter()
            .any(|scheme| arg.starts_with(&format!("{scheme}://")))
            .then(|| arg.to_string())
    })
}
