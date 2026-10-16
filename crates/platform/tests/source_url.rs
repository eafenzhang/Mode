//! `saveFile(sourceUrl)` 的**上限**断言（M3 Task 4 · DR-T4-3）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` 的 `saveFile` 两条分支。
//!
//! 本文件测的是 `platform::source_url::fetch_bounded`——命令面在 `Ok` 时才写盘，
//! 所以「超时 / 超上限 ⇒ 不写任何字节」由「不合法的结果不可能出现」保证；
//! 用例要钉的是那个判断本身：**上限在下载过程中生效**、**超时是独立的一类失败**、
//! **非 http(s) 的 scheme 在发起请求之前就被拒**。
//!
//! 服务端是**本进程里的最小 HTTP 服务**（`127.0.0.1` + 真 socket），不碰网络、不依赖外部工具。
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::Duration;

use platform::source_url::{self, FetchError, Limits};

/// 一个只服务一次请求的最小 HTTP 服务。
struct OneShotServer {
    url: String,
    handle: Option<JoinHandle<()>>,
}

impl OneShotServer {
    /// `raw_response` 是**逐字节写回**的响应；`body` 是它携带的实体。
    ///
    /// 用「逐字节写回」而不是自己拼 HTTP：用例要能造出畸形/撒谎的响应
    /// （声称 `Content-Length` 很大、或根本不给长度靠连接关闭收尾）。
    fn serve_raw(raw_response: Vec<u8>) -> Self {
        Self::serve(move |mut stream| {
            // 请求本身不关心（只发一次 GET），但必须读掉它再写，否则客户端可能拿到 RST。
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(&raw_response);
            let _ = stream.flush();
        })
    }

    /// 什么都不回（挂住），直到连接被对端关闭——超时用例的输入。
    fn hang() -> Self {
        Self::serve(|mut stream| {
            let mut buffer = [0u8; 4096];
            // 读一次请求（会正常返回），然后**不写任何响应**并挂在读上：靠客户端超时收场。
            let _ = stream.read(&mut buffer);
            std::thread::sleep(Duration::from_secs(5));
        })
    }

    fn serve<F>(handler: F) -> Self
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener.local_addr().expect("取本地端口失败").port();
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handler(stream);
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/payload"),
            handle: Some(handle),
        }
    }
}

impl Drop for OneShotServer {
    fn drop(&mut self) {
        // 服务线程是一次性的：不 join（hang 档要挂 5s，join 会把用例拖成 5s）。分离即可。
        if let Some(handle) = self.handle.take() {
            drop(handle);
        }
    }
}

fn response_with_body(body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

/// 小上限：用例要能在几 KB 上触发边界，而不是真的传 64 MiB。
const TEST_LIMITS: Limits = Limits::new(1024, Duration::from_secs(5));

#[tokio::test]
async fn a_normal_response_yields_its_bytes() {
    let payload = vec![7u8; 512];
    let server = OneShotServer::serve_raw(response_with_body(&payload));
    let fetched = source_url::fetch_bounded(&server.url, TEST_LIMITS)
        .await
        .expect("正常响应必须取回成功");
    assert_eq!(fetched, payload, "取回的字节必须与响应体逐字节一致");
}

#[tokio::test]
async fn a_body_over_the_cap_fails_instead_of_growing() {
    // ① 声明值超限：立刻失败（不下载）。
    let body = vec![1u8; 4096];
    let server = OneShotServer::serve_raw(response_with_body(&body));
    assert_eq!(
        source_url::fetch_bounded(&server.url, TEST_LIMITS).await,
        Err(FetchError::TooLarge),
        "Content-Length 已经超过上限时必须直接失败"
    );

    // ② 不给长度（靠连接关闭收尾）：**累计**判超限——这一档才是「远端不配合也吃不穿内存」。
    let mut raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
        .to_vec();
    raw.extend_from_slice(&body);
    let server = OneShotServer::serve_raw(raw);
    assert_eq!(
        source_url::fetch_bounded(&server.url, TEST_LIMITS).await,
        Err(FetchError::TooLarge),
        "没有 Content-Length 时也必须按实际累计字节判上限"
    );
}

#[tokio::test]
async fn a_stalled_response_times_out_as_its_own_failure() {
    let server = OneShotServer::hang();
    let limits = Limits::new(TEST_LIMITS.max_bytes, Duration::from_millis(400));
    assert_eq!(
        source_url::fetch_bounded(&server.url, limits).await,
        Err(FetchError::Timeout),
        "挂死的远端必须以 Timeout 结束（与「连不上」分开，见 FetchError 的文档）"
    );
}

#[tokio::test]
async fn non_http_schemes_are_rejected_before_any_request() {
    let limits = TEST_LIMITS;
    for url in [
        "file:///C:/Windows/win.ini",
        "data:text/plain;base64,aGk=",
        "ftp://example.invalid/x",
        "\\\\server\\share\\x",
        "",
        "   ",
    ] {
        assert_eq!(
            source_url::fetch_bounded(url, limits).await,
            Err(FetchError::UnsupportedScheme),
            "{url} 不是 http(s)，必须在发起请求之前被拒"
        );
    }
    assert!(source_url::is_supported_url("https://example.com/a"));
    assert!(source_url::is_supported_url("  HTTP://example.com/a  "));
    assert!(!source_url::is_supported_url("file:///C:/x"));
}

/// 产品默认值是**产品规则**，不是实现细节：改它们必须一起改这里。
///
/// 这条用例的价值在于「有人把 64 MiB 改成 4 GiB 时会红」，而不是在于数字本身。
#[test]
fn the_product_limits_are_pinned() {
    assert_eq!(
        source_url::PRODUCT_MAX_BYTES,
        64 * 1024 * 1024,
        "字节上限变了就要同时更新这条钉值与 spec 里的口径"
    );
    assert_eq!(
        source_url::PRODUCT_TIMEOUT,
        Duration::from_secs(30),
        "超时上限变了就要同时更新这条钉值与 spec 里的口径"
    );
    assert_eq!(
        Limits::PRODUCT,
        Limits::new(source_url::PRODUCT_MAX_BYTES, source_url::PRODUCT_TIMEOUT),
        "产品档必须是这两个常量的组合"
    );
}

/// 错误码是**稳定契约**（UI/日志按它分支），细节（URL、远端文案）不进码里。
#[test]
fn the_failure_codes_are_stable() {
    assert_eq!(
        FetchError::UnsupportedScheme.code(),
        "save_file_source_url_unsupported_scheme"
    );
    assert_eq!(
        FetchError::TooLarge.code(),
        "save_file_source_url_too_large"
    );
    assert_eq!(FetchError::Timeout.code(), "save_file_source_url_timeout");
    assert_eq!(FetchError::Failed.code(), "save_file_source_url_failed");
}
