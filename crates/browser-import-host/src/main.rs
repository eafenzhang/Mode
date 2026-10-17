//! Native Messaging host 入口（P1/T1）。
//!
//! 只处理**一条**消息（`chrome.runtime.sendNativeMessage` 的语义）：读请求 → 校验 →
//! 回稳定响应。T2 再把它接入 host ↔ app 的命名管道；本步不写任何 cookie。
//!
//! 纪律：stdout 只写长度前缀消息；诊断走 stderr，且不打印 cookie 值与 token。
use std::io;

use browser_import_host::{
    error_response, parse_request, read_message, success_response, write_message,
};

fn main() {
    let mut stdin = io::stdin().lock();
    let payload = match read_message(&mut stdin) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("[browser-import-host] read failed: {}", error.code());
            respond(&error_response(error.code()));
            return;
        }
    };

    match parse_request(&payload) {
        Ok(request) => {
            // T1：只确认解析成功；T2 起把 request.cookies 交给 Mode main。
            eprintln!(
                "[browser-import-host] parsed cookies={} (channel not wired yet)",
                request.cookies.len()
            );
            respond(&success_response(request.cookies.len()));
        }
        Err(error) => {
            eprintln!("[browser-import-host] invalid request: {}", error.code());
            respond(&error_response(error.code()));
        }
    }
}

fn respond(response: &serde_json::Value) {
    let payload = match serde_json::to_vec(response) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("[browser-import-host] response encode failed: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = write_message(&mut io::stdout().lock(), &payload) {
        eprintln!(
            "[browser-import-host] response write failed: {}",
            error.code()
        );
        std::process::exit(1);
    }
}
