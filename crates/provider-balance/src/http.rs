//! `ProviderBalanceFetcher` 的 reqwest 实现：真实网络取回（LT-15 P4）。
//!
//! 真源：`packages/services/src/usage-stats/providers/providerBalanceProvider.ts`
//! 的 `readApiJson` 调用（GET + `Authorization: Bearer` + 15s 超时；非 2xx 抛
//! `ApiError(status)`，2xx 但 body 不是 JSON 同样抛错）。错误映射交给
//! [`crate::service`] 的状态机：401/403 → `unauthorized`，其余 → `error/request_failed`。
//!
//! TLS：workspace 的 `reqwest` 以 `rustls-no-provider` 编译，进程级 provider 由
//! 使用者安装；这里与 `crates/platform/src/source_url.rs` 同款装 `ring`
//! （`install_default` 重复调用安全）。

use std::time::Duration;

use serde_json::Value;

use crate::service::{BalanceFuture, FetchError, ProviderBalanceFetcher};

/// TS `REQUEST_TIMEOUT_MS`：单次余额查询的墙钟超时。
pub const REQUEST_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Clone)]
pub struct ReqwestBalanceFetcher {
    client: reqwest::Client,
}

impl ReqwestBalanceFetcher {
    pub fn new() -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }
}

impl Default for ReqwestBalanceFetcher {
    fn default() -> Self {
        Self::new().expect("reqwest client")
    }
}

impl ProviderBalanceFetcher for ReqwestBalanceFetcher {
    fn fetch_balance(
        &self,
        url: &str,
        api_key: &str,
    ) -> BalanceFuture<'_, Result<Value, FetchError>> {
        let request = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json");
        Box::pin(async move {
            let response = request.send().await.map_err(|_| FetchError::Network)?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(FetchError::HttpStatus(status));
            }
            // workspace 的 reqwest 未开 json 特性：取文本再解析（语义与 TS
            // readApiJson 一致——body 不是 JSON 落 request_failed）。
            let text = response.text().await.map_err(|_| FetchError::Network)?;
            serde_json::from_str(&text).map_err(|_| FetchError::Network)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 单线程 mock HTTP server：收到请求后按预置应答写回并关闭连接。
    struct MockServer {
        url: String,
        shutdown: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn start(status_line: &'static str, body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("addr").port();
            let (shutdown, shutdown_rx) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking");
                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut buffer = [0u8; 1024];
                            let _ = stream.read(&mut buffer);
                            let response = format!(
                                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
            });
            MockServer {
                url: format!("http://127.0.0.1:{port}/balance"),
                shutdown,
                handle: Some(handle),
            }
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            let _ = self.shutdown.send(());
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[tokio::test]
    async fn fetch_returns_parsed_json_on_2xx() {
        let server = MockServer::start(
            "200 OK",
            r#"{"data":{"total_credits":10.5,"total_usage":4.0}}"#,
        );
        let fetcher = ReqwestBalanceFetcher::new().expect("fetcher");
        let payload = fetcher
            .fetch_balance(&server.url, "sk-test")
            .await
            .expect("payload");
        assert_eq!(payload["data"]["total_credits"], 10.5);
    }

    #[tokio::test]
    async fn fetch_maps_401_to_http_status() {
        let server = MockServer::start("401 Unauthorized", r#"{"error":"bad key"}"#);
        let fetcher = ReqwestBalanceFetcher::new().expect("fetcher");
        let error = fetcher
            .fetch_balance(&server.url, "sk-bad")
            .await
            .expect_err("401");
        assert_eq!(error, FetchError::HttpStatus(401));
    }

    #[tokio::test]
    async fn fetch_maps_5xx_to_http_status() {
        let server = MockServer::start("500 Internal Server Error", "oops");
        let fetcher = ReqwestBalanceFetcher::new().expect("fetcher");
        let error = fetcher
            .fetch_balance(&server.url, "sk-test")
            .await
            .expect_err("500");
        assert_eq!(error, FetchError::HttpStatus(500));
    }

    #[tokio::test]
    async fn fetch_maps_non_json_body_to_network() {
        // TS readApiJson 对 2xx 但 body 不是 JSON 同样抛错 → request_failed
        // （状态机里与 Network 同分支）。
        let server = MockServer::start("200 OK", "this is not json");
        let fetcher = ReqwestBalanceFetcher::new().expect("fetcher");
        let error = fetcher
            .fetch_balance(&server.url, "sk-test")
            .await
            .expect_err("bad json");
        assert_eq!(error, FetchError::Network);
    }

    #[tokio::test]
    async fn fetch_maps_refused_connection_to_network() {
        // 占住端口再释放：拿一个确定的空闲端口，连接应被拒绝。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://127.0.0.1:{}/balance", listener.local_addr().unwrap().port());
        drop(listener);
        let fetcher = ReqwestBalanceFetcher::new().expect("fetcher");
        let error = fetcher
            .fetch_balance(&url, "sk-test")
            .await
            .expect_err("refused");
        assert_eq!(error, FetchError::Network);
    }
}
