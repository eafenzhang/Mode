//! 契约 A（`--version`）与命令行分派。

/// 线协议令牌：broker 的**请求帧与响应帧**都用它。
///
/// 对应宿主常量 `HELPER_PROTOCOL`（`packages/desktop/src/main/windowsChromeAppBoundKey.ts:14`），
/// 宿主对响应做等值断言（同文件 391 行），不一致即判 `invalid_response`。
pub const WIRE_PROTOCOL: &str = "MODE_BROWSER_IMPORT_V1";

/// `--version` 横幅的第一字段。
///
/// 对应宿主常量 `HELPER_VERSION_PROTOCOL`（同文件 15 行），宿主在 369 行对它做等值断言。
/// **与 `WIRE_PROTOCOL` 不是同一个串**：旧产物把两者混用（横幅与回帧都发 `ZCODE_*` 品牌），
/// 实测导致宿主两处断言全部失败——这是本仓库已存在的功能缺陷，Rust 版按宿主契约修正。
pub const VERSION_PROTOCOL: &str = "MODE_BROWSER_IMPORT_HELPER";

/// 冻结的协议版本，与宿主 `HELPER_VERSION` 一致。
pub const PROTOCOL_VERSION: &str = "2";

#[derive(Debug, PartialEq)]
pub enum Mode {
    Version,
    Broker { parent_pid: u32 },
    UsageError,
}

/// 架构字段是契约的一部分（旧产物输出 `x64`），不能直接用 `std::env::consts::ARCH`
/// （它给的是 `x86_64`）。
fn arch_token() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn parse(args: &[String]) -> Mode {
    match args {
        [a] if a == "--version" => Mode::Version,
        [a, b, c] if a == "--broker" && b == "--parent-pid" => match c.parse::<u32>() {
            Ok(parent_pid) => Mode::Broker { parent_pid },
            Err(_) => Mode::UsageError,
        },
        _ => Mode::UsageError,
    }
}

/// 契约 A：制表符分隔 5 字段 `<协议名>\t<协议版本>\t<架构>\t<应用版本>\t<git 修订>`。
pub fn version_banner() -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\n",
        VERSION_PROTOCOL,
        PROTOCOL_VERSION,
        arch_token(),
        env!("CARGO_PKG_VERSION"),
        option_env!("MODE_GIT_REVISION").unwrap_or("unknown"),
    )
}
