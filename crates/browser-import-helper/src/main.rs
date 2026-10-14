use std::io::Read;

use browser_import_helper::cli::{parse, version_banner, Mode};
use browser_import_helper::protocol::{err_response, parse_request};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Mode::Version => print!("{}", version_banner()),
        Mode::Broker { parent_pid } => run_broker(parent_pid),
        Mode::UsageError => std::process::exit(2),
    }
}

fn run_broker(_parent_pid: u32) {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // 读不到请求就无法判定；不发帧（宿主会把缺帧当 invalid_response）。
        std::process::exit(2);
    }
    match parse_request(&input) {
        // 帧合法，但提权/服务通道/密钥解封尚未落地（P2）：fail-closed 且**不发帧**，
        // 不伪造契约响应。
        Ok(_request) => std::process::exit(2),
        Err(reason) => {
            print!("{}", err_response(reason));
        }
    }
}
