//! Native Messaging host 入口（P1/T2）。
//!
//! 只处理**一条**消息（`chrome.runtime.sendNativeMessage` 的语义）：读请求 → 校验 →
//! 交给 Mode main（命名管道 + 一次性 token）→ 回稳定响应。
//!
//! 纪律：stdout 只写长度前缀消息；诊断走 stderr，且不打印 cookie 值与 token。
use std::io;

use browser_import_host::{
    channel_path, deliver, deliver_cancel, error_response, load_channel_info, parse_request,
    read_message, success_response, write_message, ImportRequest, DEFAULT_HANDSHAKE_TIMEOUT,
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
        Ok(ImportRequest::Cookies(request)) => {
            eprintln!(
                "[browser-import-host] parsed cookies={}",
                request.cookies.len()
            );
            let delivered = load_channel_info(&channel_path())
                .and_then(|info| deliver(&info, &payload, DEFAULT_HANDSHAKE_TIMEOUT));
            match delivered {
                Ok(count) => respond(&success_response(count)),
                Err(error) => {
                    eprintln!("[browser-import-host] delivery failed: {}", error.code());
                    respond(&error_response(error.code()));
                }
            }
        }
        Ok(ImportRequest::Cancel) => {
            eprintln!("[browser-import-host] cancel requested");
            let delivered = load_channel_info(&channel_path())
                .and_then(|info| deliver_cancel(&info, &payload, DEFAULT_HANDSHAKE_TIMEOUT));
            match delivered {
                Ok(()) => respond(&serde_json::json!({ "ok": true })),
                Err(error) => {
                    eprintln!("[browser-import-host] cancel failed: {}", error.code());
                    respond(&error_response(error.code()));
                }
            }
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
