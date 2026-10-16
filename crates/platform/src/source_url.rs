//! `saveFile` 的 `sourceUrl` 分支要用的**有上限的 HTTP 取回面**（M3 Task 4 · DR-T4-3）。
//!
//! ## 为什么单独一层，而且上限是产品规则
//!
//! 这条路径的形态是「**任意远端**让应用写一个文件」：URL 由渲染层给出，内容由远端决定。
//! 没有上限时，一个（或一串）远端就能让应用把任意大的字节写进用户选的路径。所以**超时**与
//! **字节上限**不是实现细节，而是这条路径的产品规则：
//!
//! - 两者是**公开常量**（[`PRODUCT_MAX_BYTES`] / [`PRODUCT_TIMEOUT`]），改它们必须改用例；
//! - 上限在**下载过程中**生效（每读一块累计判断），不是「读完再看长度」——
//!   后者在恶意/畸形响应下已经把内存吃完了；
//! - 声明的 `Content-Length` 先判一次（超限就直接失败，不下载）。
//!
//! ## 只收 http/https
//!
//! `file://` / `data:` / `ftp://` 之类的 scheme 一律拒绝：这条路径的语义是「从远端取一个文件」，
//! 而 `file://` 会把「远端取回」变成「本地任意文件读取 + 写到用户选的路径」，
//! `data:` 则给出另一条没有网络语义的注入面。拒绝发生在**发起请求之前**。
//!
//! ## TLS provider 为什么要自己装
//!
//! 本工作空间的 `reqwest` 以 `rustls-no-provider` 档编译（`tauri-plugin-updater` 的选择），
//! 也就是「rustls 在，但进程级 provider 要由使用者安装」。这里按同样的做法装 `ring`
//! （`install_default` 重复调用是安全的：已装时返回 `Err`，忽略即可）。
use std::time::Duration;

use futures_util::StreamExt;

/// 单次取回的**字节上限**（产品默认）：64 MiB。
///
/// 量级依据：这条分支服务的是附件/导出文件之类的下载，不是媒体库；而它唯一的上游是渲染层给出的
/// URL，所以按「够用且不会把内存/磁盘吃穿」取值。改它必须同时改
/// `crates/platform/tests/source_url.rs` 的钉值用例。
pub const PRODUCT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 单次取回的**墙钟超时**（产品默认，含连接、首字节与整体传输）。
///
/// 量级依据：用户在对话框里已经选好了路径，超时之后他还要等这个结果；30s 是「慢网络还能成、
/// 挂死的远端不会把对话框后的等待拖到无限」的折中。改它必须同时改钉值用例。
pub const PRODUCT_TIMEOUT: Duration = Duration::from_secs(30);

/// 单次取回的上限集（超时 + 字节上限）。
///
/// 用例用小的值驱动边界（例如 128 字节 / 300ms），产品用 [`Limits::PRODUCT`]——两者走**同一段
/// 实现**，所以「用例测的」与「产品跑的」是同一条路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub timeout: Duration,
}

impl Limits {
    /// 产品默认档（见两个常量）。
    pub const PRODUCT: Limits = Limits {
        max_bytes: PRODUCT_MAX_BYTES,
        timeout: PRODUCT_TIMEOUT,
    };

    /// 显式构造（用例与将来可能需要按设置项调整时用）。
    pub const fn new(max_bytes: u64, timeout: Duration) -> Self {
        Self {
            max_bytes,
            timeout,
        }
    }
}

/// 取回失败的原因。**每个变体对应一个稳定的错误码**（[`FetchError::code`]），
/// 命令面把它放进 `SaveFileResult.error`，细节不上抛（细节只进诊断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// scheme 不是 http/https（见模块文档：这条路径不做本地文件读取）。
    UnsupportedScheme,
    /// 超过字节上限（声明值或实际累计值）。
    TooLarge,
    /// 超过墙钟超时。
    Timeout,
    /// 连接失败、非 2xx、传输中断等其余情况。
    Failed,
}

impl FetchError {
    /// 稳定的错误码（UI/日志按它区分结局；文案改动不会影响它）。
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedScheme => "save_file_source_url_unsupported_scheme",
            Self::TooLarge => "save_file_source_url_too_large",
            Self::Timeout => "save_file_source_url_timeout",
            Self::Failed => "save_file_source_url_failed",
        }
    }
}

/// URL 是不是本路径接受的目标（http/https）。
pub fn is_supported_url(url: &str) -> bool {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// 有上限地取回一份远端内容。
///
/// 失败一律返回 [`FetchError`]，**不返回部分内容**——调用方（命令面）只在 `Ok` 时才写盘，
/// 所以「失败」与「写了一半」在结构上不可能同时发生。
pub async fn fetch_bounded(url: &str, limits: Limits) -> Result<Vec<u8>, FetchError> {
    if !is_supported_url(url) {
        return Err(FetchError::UnsupportedScheme);
    }
    install_crypto_provider();

    let client = reqwest::Client::builder()
        .timeout(limits.timeout)
        .build()
        .map_err(|_| FetchError::Failed)?;

    let response = client
        .get(url.trim())
        .send()
        .await
        .map_err(|error| classify(&error))?;

    if !response.status().is_success() {
        return Err(FetchError::Failed);
    }

    // 先看声明值：远端自己说了超限就立刻失败，不必下载（也避免「读到一半才发现」）。
    if let Some(declared) = response.content_length() {
        if declared > limits.max_bytes {
            return Err(FetchError::TooLarge);
        }
    }

    // 再按**实际**累计判：声明值可以缺席（`Connection: close`）也可以撒谎。
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| classify(&error))?;
        let next_len = body.len() as u64 + chunk.len() as u64;
        if next_len > limits.max_bytes {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 把 reqwest 的错误收敛成稳定的四类。超时是**单独一类**：它与「连不上」对用户含义不同
/// （一个是远端慢，一个是远端不可用），也让用例能把超时钉住而不是混进 `Failed`。
fn classify(error: &reqwest::Error) -> FetchError {
    if error.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Failed
    }
}

/// 装进程级 TLS provider（重复调用安全）。见模块文档「TLS provider 为什么要自己装」。
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
