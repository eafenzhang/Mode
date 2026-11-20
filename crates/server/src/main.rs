//! server-mode 完整装配：健康/版本 + LAN 管理面（credential-store 凭据）+
//! `/ws/host` RPC 帧桥（LT-15 P2/P4）。
//!
//! LAN 通道默认不开（`LanPairing::load` 读不到凭据即 disabled 视图）；
//! `/api/lan/enable` 持久化开关后监听面由宿主进程管理（本入口只挂管理路由）。

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("MODE_SERVER_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3031);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("mode-server listening on http://127.0.0.1:{port}");

    let data_root = data_root::resolve_data_root_dir(
        &std::env::var("MODE_BASE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
            }),
    );
    let config = mode_server::server_info::ServerInfoConfig::from_env();
    let router = match mode_server::lan::lan_pairing_from_store(&data_root, None) {
        Ok(pairing) => mode_server::router_with_lan(config, pairing)
            .merge(mode_server::router_with_ws_host(
                mode_server::server_info::ServerInfoConfig::from_env(),
                data_root,
            )),
        Err(error) => {
            eprintln!("lan pairing store unavailable: {error}; lan routes omitted");
            mode_server::router_with_ws_host(config, data_root)
        }
    };
    axum::serve(listener, router).await
}
