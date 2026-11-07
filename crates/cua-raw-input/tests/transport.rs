//! CUA broker socket 传输：真实平台 socket 的服务端替身。

use std::time::Duration;

use cua_raw_input::transport::{
    BrokerClient, BrokerError, HealthProbeOptions,
};
use serde_json::json;

const SUCCESS: &[u8] = b"{\"ok\":true,\"result\":{\"pong\":true}}\n";
const REMOTE_ERROR: &[u8] = b"{\"ok\":false,\"error\":{\"code\":\"boom\",\"message\":\"bad\"}}\n";
const INVALID_JSON: &[u8] = b"not-json\n";
const HEALTH: &[u8] = b"{\"ok\":true,\"result\":{\"bundleId\":\"com.example.cua\",\"pid\":123}}\n";
const RAW_INPUT: &[u8] = b"{\"ok\":true,\"result\":{\"dispatched\":2.5}}
";

#[cfg(windows)]
fn unique_path() -> String {
    format!(
        r"\\.\pipe\mode-cua-transport-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[cfg(unix)]
fn unique_path() -> String {
    std::env::temp_dir()
        .join(format!(
            "mode-cua-transport-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn start_server(response: &'static [u8], delay: Duration) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    let path = unique_path();
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&path)
        .expect("create named pipe");
    tokio::spawn(async move {
        server.connect().await.expect("client connect");
        let mut reader = BufReader::new(server);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        tokio::time::sleep(delay).await;
        let mut stream = reader.into_inner();
        let _ = stream.write_all(response).await;
        let _ = stream.flush().await;
    });
    path
}

#[cfg(unix)]
fn start_server(response: &'static [u8], delay: Duration) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let path = unique_path();
    let listener = tokio::net::UnixListener::bind(&path).expect("bind unix socket");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        tokio::time::sleep(delay).await;
        let mut stream = reader.into_inner();
        let _ = stream.write_all(response).await;
        let _ = stream.flush().await;
    });
    path
}

fn cleanup(path: &str) {
    #[cfg(unix)]
    let _ = std::fs::remove_file(path);
    #[cfg(windows)]
    let _ = path;
}

fn assert_code(error: BrokerError, code: &str) {
    assert_eq!(error.code, code, "unexpected error: {error}");
}

#[tokio::test]
async fn call_returns_result() {
    let path = start_server(SUCCESS, Duration::ZERO);
    let result = BrokerClient::new(&path)
        .call("health", json!({}), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result, json!({ "pong": true }));
    cleanup(&path);
}

#[tokio::test]
async fn call_maps_remote_error() {
    let path = start_server(REMOTE_ERROR, Duration::ZERO);
    let error = BrokerClient::new(&path)
        .call("observe", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert_code(error.clone(), "boom");
    assert_eq!(error.message, "bad");
    cleanup(&path);
}

#[tokio::test]
async fn call_maps_invalid_json_to_internal() {
    let path = start_server(INVALID_JSON, Duration::ZERO);
    let error = BrokerClient::new(&path)
        .call("health", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert_code(error, "internal");
    cleanup(&path);
}

#[tokio::test]
async fn call_times_out() {
    let path = start_server(SUCCESS, Duration::from_secs(5));
    let error = BrokerClient::new(&path)
        .call("health", json!({}), Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_code(error, "timeout");
    cleanup(&path);
}

#[tokio::test]
async fn probe_health_reads_bundle_and_pid() {
    let path = start_server(HEALTH, Duration::ZERO);
    let health = BrokerClient::new(&path)
        .probe_health(HealthProbeOptions {
            timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(20),
            per_try_timeout: Duration::from_millis(500),
        })
        .await
        .unwrap();
    assert_eq!(health.bundle_id.as_deref(), Some("com.example.cua"));
    assert_eq!(health.pid, Some(123));
    cleanup(&path);
}

#[tokio::test]
async fn probe_health_exhausts_deadline() {
    let path = unique_path();
    let error = BrokerClient::new(&path)
        .probe_health(HealthProbeOptions {
            timeout: Duration::from_millis(80),
            poll_interval: Duration::from_millis(20),
            per_try_timeout: Duration::from_millis(20),
        })
        .await
        .unwrap_err();
    assert_code(error, "broker_unavailable");
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broker_call_adapter_parses_dispatched() {
    let path = start_server(RAW_INPUT, Duration::ZERO);
    let handle = tokio::runtime::Handle::current();
    let call = cua_raw_input::broker_call(handle);
    let method_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        call(method_path, vec![json!({ "k": "move", "x": 1 })], 2_000)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.dispatched, Some(2.5));
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broker_call_adapter_maps_remote_error() {
    let path = start_server(REMOTE_ERROR, Duration::ZERO);
    let handle = tokio::runtime::Handle::current();
    let call = cua_raw_input::broker_call(handle);
    let method_path = path.clone();
    let error = tokio::task::spawn_blocking(move || {
        call(method_path, vec![json!({ "k": "move" })], 2_000)
    })
    .await
    .unwrap()
    .unwrap_err();
    match error {
        cua_raw_input::injector::BrokerCallError::Broker { code, message } => {
            assert_eq!(code, "boom");
            assert_eq!(message, "bad");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    cleanup(&path);
}
