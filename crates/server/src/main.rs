//! 最小可运行 axum 服务：健康/版本面。业务路由在 LT-9 后续任务接入。

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("MODE_SERVER_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3031);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("mode-server skeleton listening on http://127.0.0.1:{port}");
    axum::serve(listener, mode_server::router()).await
}
