use browser_import_helper::cli::{parse, version_banner, Mode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Mode::Version => print!("{}", version_banner()),
        // 契约 B 尚未实现：fail-closed，退出码与用法错误一致，不吐 OK 帧。
        Mode::Broker { .. } => std::process::exit(2),
        Mode::UsageError => std::process::exit(2),
    }
}
