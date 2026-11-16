//! 进程启动：解析 `app-server --stdio`，构建 tokio runtime，把 stdio 接到
//! [`crate::run_stdio`]。
//!
//! 只支持 `app-server --stdio` 这一种运行形态；其余参数忽略但记录到 stderr
//! （`agent-process` 会收集并广播 stderr）。即使首个参数不是 `app-server`，仍按
//! stdio 服务启动：二进制没有第二种模式，不因多余参数拒绝服务。

use std::process::ExitCode;
use std::sync::Arc;

use crate::dispatch::AgentServer;
use crate::server::run_stdio;

pub const APP_SERVER_MODE: &str = "app-server";
pub const STDIO_FLAG: &str = "--stdio";

/// 启动参数的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupArgs {
    /// 是否匹配受支持的 `app-server --stdio` 形态。
    pub supported: bool,
    /// 不认识的参数（已从受支持的 token 中剔除），只用于记录。
    pub ignored: Vec<String>,
}

/// 解析启动参数。受支持形态是首个参数 `app-server` + 任意位置的 `--stdio`；
/// 其余 token 都进 `ignored`（`--stdio` 的重复出现也忽略）。
pub fn parse_startup_args(args: &[String]) -> StartupArgs {
    let supported = args.first().map(String::as_str) == Some(APP_SERVER_MODE)
        && args.iter().skip(1).any(|arg| arg == STDIO_FLAG);
    let ignored = args
        .iter()
        .enumerate()
        .filter(|(index, arg)| {
            !(*index == 0 && arg.as_str() == APP_SERVER_MODE) && arg.as_str() != STDIO_FLAG
        })
        .map(|(_, arg)| arg.clone())
        .collect();
    StartupArgs { supported, ignored }
}

/// 解析 `std::env::args()`（跳过程序名）并运行 stdio 服务。
pub fn run_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let startup = parse_startup_args(&args);
    if !startup.supported {
        eprintln!(
            "agent-server: expected `app-server --stdio`; starting stdio anyway (args: {args:?})"
        );
    }
    if !startup.ignored.is_empty() {
        eprintln!("agent-server: ignoring arguments: {:?}", startup.ignored);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("agent-server: failed to start tokio runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async {
        let server = Arc::new(AgentServer::new());
        match run_stdio(tokio::io::stdin(), tokio::io::stdout(), server).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("agent-server: {error}");
                ExitCode::FAILURE
            }
        }
    })
}
