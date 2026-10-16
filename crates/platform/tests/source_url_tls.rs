//! `saveFile(sourceUrl)` 的 **TLS 路径**断言（M3 Task 4 fix round · Ruling 30）。
//!
//! ## 为什么单独一个文件
//!
//! `source_url.rs` 那一套只对 `127.0.0.1` 讲**明文 HTTP**，于是 `install_crypto_provider()`
//! 与整条 rustls 握手**编译得过但从未运行**。而真实的 `sourceUrl` 几乎总是 `https://`
//! ——也就是说最可能发生的调用恰恰是零运行证据的那条路径，失败还只会表现为
//! `save_file_source_url_failed`（CI 一片绿）。
//!
//! 这里用一个**自签证书的本地 TLS 服务**把那条路径真的跑起来：连接、握手、证书校验全都会发生。
//!
//! ## 能证什么、不能证什么（写清，不让「有 HTTP 测试」读成「HTTPS 验过了」）
//!
//! **能证**：
//!
//! 1. `fetch_bounded` 真的走 TLS（客户端发出 ClientHello、服务端完成握手协商）；
//! 2. **进程级 TLS provider 被产品代码装上了**：服务端用的是**显式** provider
//!    （`builder_with_provider`），所以进程默认 provider 只可能由 `fetch_bounded` 装；
//!    没装的话 rustls 报的是另一句话（"no process-level CryptoProvider"），而下面的断言
//!    专门排除那一种；
//! 3. 失败发生在**证书校验**这一步（自签证书不在系统信任库里）——这正是真实世界里
//!    「证书不对」的处置，且它被归成稳定的 `Failed` 码（不是超时、不是连接失败）。
//!
//! **不能证（已登记，成本写在 spec 里）**：一次**成功**的 HTTPS 往返。本函数的客户端只信任
//! 系统根（`rustls-platform-verifier`），要让本地自签服务被接受，只有两条路——给产品代码加
//! 「注入信任根」的测试钩子（改产品行为），或者把自签根装进**系统信任库**
//! （Windows 上 `certutil` 装 Root 会弹安全确认，无人值守环境不可接受）。两条都不做，
//! 所以成功档由 opt-in 的真网络探针覆盖（见下面 `real_https_probe`）。
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use platform::source_url::{self, FetchError, Limits};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// 自签证书 + 本地 TLS 服务（只服务一次连接）。
struct TlsServer {
    url: String,
    handle: Option<JoinHandle<()>>,
}

impl TlsServer {
    fn start() -> Self {
        let certified = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .expect("生成自签证书失败");
        let cert = CertificateDer::from(certified.cert.der().to_vec());
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));

        // **显式** provider（不走进程默认档）：见文件头第 2 条——进程默认 provider 必须由
        // 被测代码去装，否则这条用例就是在替它作弊。
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("协议版本配置失败")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("服务端证书配置失败");
        let config = Arc::new(config);

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener.local_addr().expect("取本地端口失败").port();
        let handle = std::thread::spawn(move || {
            let Ok((tcp, _)) = listener.accept() else {
                return;
            };
            serve_once(config, tcp);
        });
        Self {
            url: format!("https://127.0.0.1:{port}/payload"),
            handle: Some(handle),
        }
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        // 一次性线程：不 join（客户端可能提前放弃握手），分离即可。
        if let Some(handle) = self.handle.take() {
            drop(handle);
        }
    }
}

/// 握一次手、读一次请求、回一个固定响应。客户端因证书被拒时会在握手阶段断开——
/// 这里的读/写失败都是**预期**的，不 panic。
fn serve_once(config: Arc<rustls::ServerConfig>, tcp: TcpStream) {
    let Ok(mut connection) = rustls::ServerConnection::new(config) else {
        return;
    };
    let mut tcp = tcp;
    let mut stream = rustls::Stream::new(&mut connection, &mut tcp);
    let mut buffer = [0u8; 4096];
    if stream.read(&mut buffer).is_err() {
        return;
    }
    let body = b"tls-payload";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

const TEST_LIMITS: Limits = Limits::new(1024, Duration::from_secs(10));

#[tokio::test]
async fn a_self_signed_https_endpoint_fails_at_certificate_validation_with_a_provider_installed() {
    let server = TlsServer::start();
    let failure = source_url::fetch_bounded_detailed(&server.url, TEST_LIMITS)
        .await
        .expect_err("自签证书不在系统信任库里，必须失败（否则这条用例什么都没证）");
    println!("[tls-probe] kind={:?} detail={}", failure.kind, failure.detail);
    let detail = failure.detail.to_lowercase();

    // ① 归成稳定码：证书问题不是超时、也不是「连不上」的分类错位。
    assert_eq!(
        failure.kind,
        FetchError::Failed,
        "证书校验失败必须归成 `Failed`（细节在 detail 里）：{}",
        failure.detail
    );
    // ② **provider 装上了**：没装的话 rustls 在构造客户端前就报
    //    "no process-level CryptoProvider"（那句话里没有证书/issuer 之类的词）。
    assert!(
        !detail.contains("cryptoprovider") && !detail.contains("crypto provider"),
        "失败原因不能是「没装进程级 TLS provider」——那说明 install_crypto_provider 没生效：{}",
        failure.detail
    );
    // ③ 失败发生在**证书校验**：自签证书的典型症状是「unknown issuer / invalid peer certificate」。
    assert!(
        detail.contains("certificate") || detail.contains("issuer") || detail.contains("unknown"),
        "失败原因应当是证书校验（而不是连接/DNS/协议错误）：{}",
        failure.detail
    );
    // ④ 同一进程里直接问 rustls：进程默认 provider 确实被装上了（② 的正面证据）。
    assert!(
        rustls::crypto::CryptoProvider::get_default().is_some(),
        "fetch_bounded 应当装好进程级 TLS provider（这是所有 https 请求的前提）"
    );
}

/// 稳定码与细节的**分工**（Ruling 30 的副产品）：码跨端、细节只进诊断。
#[test]
fn the_failure_detail_is_carried_but_the_code_stays_stable() {
    // `fetch_bounded` 是命令面用的那一层：它只回稳定码，细节不跨端。
    let failing = source_url::FetchError::Failed;
    assert_eq!(failing.code(), "save_file_source_url_failed");
    // 细节类型的存在性由上面那条 TLS 用例使用（编译期即证）；这里钉住它的形状契约：
    // `FetchFailure` 的两个字段都是公开只读语义（`kind` 用于分支、`detail` 用于日志）。
    let sample = source_url::FetchFailure {
        kind: FetchError::TooLarge,
        detail: "declared 9999 > cap 1024".to_string(),
    };
    assert_eq!(sample.kind.code(), "save_file_source_url_too_large");
    assert!(sample.detail.contains("cap"));
}

/// **opt-in 真网络探针**：一次真的 HTTPS 往返（成功档）。
///
/// 为什么必须 opt-in：它要走外网（CI 不可依赖），而「成功档」在本地自签服务上做不到
/// （见文件头）。默认 `#[ignore]`——「没跑」在测试输出里必须看得出来，不能报 ok。
///
/// ```bash
/// MODE_TAURI_HTTPS_PROBE=1 node scripts/rust-cargo.ts test -p platform --test source_url_tls -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "opt-in：要真网络（MODE_TAURI_HTTPS_PROBE=1 才跑，见 spec 的覆盖分工）"]
async fn real_https_probe_over_the_public_internet() {
    if std::env::var("MODE_TAURI_HTTPS_PROBE").as_deref() != Ok("1") {
        eprintln!("[https-probe] 跳过：设 MODE_TAURI_HTTPS_PROBE=1 才跑（要走外网）");
        return;
    }
    let url = std::env::var("MODE_TAURI_HTTPS_PROBE_URL")
        .unwrap_or_else(|_| "https://example.com/".to_string());
    let limits = Limits::new(source_url::PRODUCT_MAX_BYTES, Duration::from_secs(30));
    let body = source_url::fetch_bounded(&url, limits)
        .await
        .unwrap_or_else(|error| panic!("真 HTTPS 往返必须成功（{}）：{url}", error.code()));
    println!("[https-probe] url={url} bytes={}", body.len());
    assert!(!body.is_empty(), "HTTPS 响应体不该是空的");
}
