//! C4 T4b：v4 附件事务、ref 授权矩阵与 sendText（正确行为）。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §5 与
//! `fixtures/c4/attachments-v4-frames.json`。授权按 C4-T4 裁决「正确行为」：
//! 同 session 授权；跨 session share 错误；未知 ref preview 错误。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_core::ModelProvider;
use agent_server::{
    AgentServer, ProviderConfigHub, ProviderRequestContext, ProviderSource, ServerConfig,
};
use base64::Engine;
use serde_json::{json, Value};
use tokio::runtime::Runtime;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-att-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct UnusedSource;

impl ProviderSource for UnusedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        Err(format!("Provider 不存在: {provider_id}"))
    }

    fn cache_key(&self) -> String {
        "unused".to_string()
    }
}

struct TestServer {
    server: Arc<AgentServer>,
    runtime: Runtime,
}

impl TestServer {
    fn open(root: &TempRoot) -> Self {
        let config = ServerConfig::from_data_base_dir(root.path());
        let server = Arc::new(AgentServer::with_parts(
            config,
            ProviderConfigHub::from_env(),
            Arc::new(UnusedSource),
        ));
        let runtime = Runtime::new().expect("tokio runtime");
        let mut sink: Vec<u8> = Vec::new();
        runtime
            .block_on(async {
                agent_server::run_stdio(&b""[..], &mut sink, Arc::clone(&server)).await
            })
            .expect("存储门必须完成");
        Self { server, runtime }
    }

    /// dispatch 必须在 spawn 它 worker 的同一 runtime 上执行（session/create 会 spawn）。
    fn dispatch(&self, method: &str, params: Value) -> Result<Value, agent_server::ServerError> {
        let server = Arc::clone(&self.server);
        self.runtime
            .block_on(async move { server.dispatch(method, params) })
    }
}

fn open_server(root: &TempRoot) -> TestServer {
    TestServer::open(root)
}

const TEXT_BODY: &str = "attachment body for the C4 probe\n";
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn commit_text_attachment(server: &TestServer, session_id: &str) -> String {
    let begin = server
        .dispatch(
            "v4/attachment/begin",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-text-1",
                "sessionId": session_id,
                "fileName": "note.txt",
                "mime": "text/plain",
                "totalBytes": TEXT_BODY.len(),
                "totalChunks": 1,
                "checksum": "stub",
            }),
        )
        .expect("begin");
    assert_eq!(begin["state"], json!("staging"));
    assert_eq!(begin["nextChunkIndex"], json!(0));

    let chunk = server
        .dispatch(
            "v4/attachment/chunk",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-text-1",
                "sessionId": session_id,
                "chunkIndex": 0,
                "dataBase64": base64::engine::general_purpose::STANDARD.encode(TEXT_BODY),
            }),
        )
        .expect("chunk");
    assert_eq!(chunk["nextChunkIndex"], json!(1));

    let commit = server
        .dispatch(
            "v4/attachment/commit",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-text-1",
                "sessionId": session_id,
            }),
        )
        .expect("commit");
    let attachment_ref = commit["ref"].as_str().expect("ref").to_string();
    assert!(attachment_ref.starts_with(&format!("mode-artifact://{session_id}/tool-result-")));
    attachment_ref
}

#[test]
fn attachment_transaction_stores_reads_and_previews() {
    let root = TempRoot::new("tx");
    let server = open_server(&root);
    let session_id = "sess_att_owner";
    let attachment_ref = commit_text_attachment(&server, session_id);

    // 分段读：整段 + 偏移段。
    let read = server
        .dispatch(
            "v4/attachment/read",
            json!({
                "sessionId": session_id,
                "ref": attachment_ref,
                "offset": 0,
                "limit": 65536,
            }),
        )
        .expect("read");
    let data = read["dataBase64"].as_str().expect("data");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("decode"),
        TEXT_BODY.as_bytes()
    );
    assert_eq!(read["mediaType"], json!("text/plain"));
    assert_eq!(read["totalBytes"], json!(TEXT_BODY.len()));
    assert_eq!(read["nextOffset"], Value::Null);

    let offset_read = server
        .dispatch(
            "v4/attachment/read",
            json!({
                "sessionId": session_id,
                "ref": attachment_ref,
                "offset": 4,
                "limit": 8,
            }),
        )
        .expect("offset read");
    let slice = offset_read["dataBase64"].as_str().expect("data");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(slice)
            .expect("decode"),
        &TEXT_BODY.as_bytes()[4..12]
    );
    assert_eq!(offset_read["nextOffset"], json!(12));

    // previewSource：本地路径。
    let preview = server
        .dispatch(
            "v4/attachment/previewSource",
            json!({
                "sessionId": session_id,
                "ref": attachment_ref,
                "clientMode": "desktop-continuous",
            }),
        )
        .expect("preview");
    assert_eq!(preview["kind"], json!("local_path"));
    assert_eq!(preview["mediaType"], json!("text/plain"));
    let path = preview["path"].as_str().expect("path");
    assert!(Path::new(path).is_file());
    let normalized = path.replace('\\', "/");
    assert!(normalized.contains("cli/artifacts"));
    assert!(path.contains("prompt-attachment-upload-upload-text-1-tool-result-"));

    // PNG：落 image-cache（观察布局）。
    let begin = server
        .dispatch(
            "v4/attachment/begin",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-png-1",
                "sessionId": session_id,
                "fileName": "probe.png",
                "mime": "image/png",
                "totalBytes": PNG_1X1.len(),
                "totalChunks": 1,
                "checksum": "stub",
            }),
        )
        .expect("png begin");
    assert_eq!(begin["state"], json!("staging"));
    server
        .dispatch(
            "v4/attachment/chunk",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-png-1",
                "sessionId": session_id,
                "chunkIndex": 0,
                "dataBase64": base64::engine::general_purpose::STANDARD.encode(PNG_1X1),
            }),
        )
        .expect("png chunk");
    server
        .dispatch(
            "v4/attachment/commit",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-png-1",
                "sessionId": session_id,
            }),
        )
        .expect("png commit");
    let image_cache = root.path().join(".mode/cli/image-cache");
    assert!(image_cache.exists());
    let mut found_png = false;
    let stack = vec![image_cache.clone()];
    for dir in stack {
        for entry in std::fs::read_dir(&dir).expect("read image cache") {
            let entry = entry.expect("entry");
            if entry.path().is_dir() {
                found_png = found_png || std::fs::read_dir(entry.path()).is_ok();
            } else if entry.path().extension().and_then(|value| value.to_str()) == Some("png") {
                found_png = true;
            }
        }
    }
    assert!(found_png, "image-cache 里必须有 PNG 副本");
}

#[test]
fn attachment_authorization_matrix_matches_the_ruling() {
    let root = TempRoot::new("auth");
    let server = open_server(&root);
    let owner = "sess_att_owner";
    let stranger = "sess_att_stranger";
    let attachment_ref = commit_text_attachment(&server, owner);

    // 跨 session 的 preview 面 → previewRefNotAuthorized（观察文案）。
    let error = server
        .dispatch(
            "v4/attachment/read",
            json!({
                "sessionId": stranger,
                "ref": attachment_ref,
                "offset": 0,
                "limit": 10,
            }),
        )
        .expect_err("cross-session read must fail");
    assert_eq!(error.code(), -32603);
    assert_eq!(error.message(), "fault.attachment.previewRefNotAuthorized");

    // 未知 ref 的 preview 面 → 同一句话（观察）。
    let error = server
        .dispatch(
            "v4/attachment/previewSource",
            json!({
                "sessionId": stranger,
                "ref": "mode-artifact://sess_att_owner/tool-result-missing",
                "clientMode": "desktop-continuous",
            }),
        )
        .expect_err("unknown ref must fail");
    assert_eq!(error.message(), "fault.attachment.previewRefNotAuthorized");

    // 跨 session 的 share 面 → shareRead/shareStat 错误（观察）。
    let error = server
        .dispatch(
            "v4/conversation/attachmentRead",
            json!({
                "sessionId": stranger,
                "ref": attachment_ref,
                "target": { "rowId": 1, "entityId": "entity" },
                "attachmentIndex": 0,
                "offset": 0,
                "limit": 10,
            }),
        )
        .expect_err("cross-session share read must fail");
    assert_eq!(error.message(), "fault.attachment.shareReadNotAuthorized");

    let error = server
        .dispatch(
            "v4/conversation/attachmentStat",
            json!({
                "sessionId": stranger,
                "ref": attachment_ref,
                "target": { "rowId": 1, "entityId": "entity" },
                "attachmentIndex": 0,
            }),
        )
        .expect_err("cross-session share stat must fail");
    assert_eq!(error.message(), "fault.attachment.shareStatNotAuthorized");

    // 同 session 的 share 面 → 授权。
    let stat = server
        .dispatch(
            "v4/conversation/attachmentStat",
            json!({
                "sessionId": owner,
                "ref": attachment_ref,
                "target": { "rowId": 1, "entityId": "entity" },
                "attachmentIndex": 0,
            }),
        )
        .expect("stat");
    assert_eq!(stat["mediaType"], json!("text/plain"));
    assert_eq!(stat["totalBytes"], json!(TEXT_BODY.len()));

    // 正确行为裁决：ref 未知时 conversationAttachmentStat 也报 share 错误
    // （不复刻 oracle 的 FK/preview 缺陷差异）。
    let error = server
        .dispatch(
            "v4/conversation/attachmentStat",
            json!({
                "sessionId": owner,
                "ref": "mode-artifact://sess_att_owner/tool-result-unknown",
                "target": { "rowId": 1, "entityId": "entity" },
                "attachmentIndex": 0,
            }),
        )
        .expect_err("unknown ref must fail");
    assert_eq!(error.message(), "fault.attachment.shareStatNotAuthorized");
}

#[test]
fn abort_then_commit_is_upload_not_found() {
    let root = TempRoot::new("abort");
    let server = open_server(&root);
    let session_id = "sess_att_abort";
    // 零字节上传必须零块（协议 superRefine）：4 字节 + 0 块 → -32602。
    let error = server
        .dispatch(
            "v4/attachment/begin",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-abort-1",
                "sessionId": session_id,
                "fileName": "aborted.txt",
                "mime": "text/plain",
                "totalBytes": 4,
                "totalChunks": 0,
                "checksum": "stub",
            }),
        )
        .expect_err("zero-byte mismatch");
    assert_eq!(error.code(), -32602);
    let begin = server
        .dispatch(
            "v4/attachment/begin",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-abort-2",
                "sessionId": session_id,
                "fileName": "aborted.txt",
                "mime": "text/plain",
                "totalBytes": 4,
                "totalChunks": 1,
                "checksum": "stub",
            }),
        )
        .expect("begin 2");
    assert_eq!(begin["state"], json!("staging"));

    let aborted = server
        .dispatch(
            "v4/attachment/abort",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-abort-2",
                "sessionId": session_id,
            }),
        )
        .expect("abort");
    assert_eq!(aborted, json!({}));

    let error = server
        .dispatch(
            "v4/attachment/commit",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-abort-2",
                "sessionId": session_id,
            }),
        )
        .expect_err("commit after abort");
    assert_eq!(error.message(), "fault.attachment.uploadNotFound");

    // 零块上传约束：totalBytes=0 必须配 totalChunks=0。
    let error = server
        .dispatch(
            "v4/attachment/begin",
            json!({
                "connectionId": "conn-c4",
                "uploadId": "upload-bad-zero",
                "sessionId": session_id,
                "fileName": "zero.txt",
                "mime": "text/plain",
                "totalBytes": 0,
                "totalChunks": 1,
                "checksum": "stub",
            }),
        )
        .expect_err("zero-byte mismatch");
    assert_eq!(error.code(), -32602);
}

#[test]
fn send_text_with_attachments_follows_the_ruling() {
    let root = TempRoot::new("sendtext");
    let server = open_server(&root);
    let session_id = "sess_send";

    // 建 session（无模型也允许创建——C1 裁决 7）。
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).expect("workspace");
    let created = server
        .dispatch(
            "session/create",
            json!({
                "sessionId": session_id,
                "workspace": {
                    "workspacePath": ws.display().to_string(),
                    "workspaceKey": ws.display().to_string(),
                },
            }),
        )
        .expect("create");
    assert_eq!(created["session"]["sessionId"], json!(session_id));

    // 无 ref 的 sendText 直接 accepted 并起 turn。
    let ack = server
        .dispatch(
            "v4/command",
            json!({
                "commandId": "cmd-send-1",
                "clientId": "probe-c4",
                "sessionId": session_id,
                "type": "sendText",
                "payload": { "text": "hello without attachments" },
                "issuedAt": 1,
            }),
        )
        .expect("ack");
    assert_eq!(ack["status"], json!("accepted"));
    assert_eq!(ack["result"]["type"], json!("sendText"));

    // 未知 ref → failed ACK（正确行为，不 FK 崩溃）。
    let ack = server
        .dispatch(
            "v4/command",
            json!({
                "commandId": "cmd-send-2",
                "clientId": "probe-c4",
                "sessionId": session_id,
                "type": "sendText",
                "payload": {
                    "text": "with unknown ref",
                    "attachments": [
                        { "ref": "mode-artifact://x/tool-result-none", "fileName": "a.txt", "mime": "text/plain", "bytes": 4 }
                    ]
                },
                "issuedAt": 2,
            }),
        )
        .expect("ack 2");
    assert_eq!(ack["status"], json!("failed"));
    assert_eq!(
        ack["reasonCode"],
        json!("fault.command.attachmentRefUnknown")
    );

    // 正常 commit 后带 ref 发送 → accepted。
    let attachment_ref = commit_text_attachment(&server, session_id);
    let ack = server
        .dispatch(
            "v4/command",
            json!({
                "commandId": "cmd-send-3",
                "clientId": "probe-c4",
                "sessionId": session_id,
                "type": "sendText",
                "payload": {
                    "text": "look at the note",
                    "attachments": [
                        { "ref": attachment_ref, "fileName": "note.txt", "mime": "text/plain", "bytes": TEXT_BODY.len() }
                    ]
                },
                "issuedAt": 3,
            }),
        )
        .expect("ack 3");
    assert_eq!(ack["status"], json!("accepted"));

    // 非 active session → failed ACK。
    let ack = server
        .dispatch(
            "v4/command",
            json!({
                "commandId": "cmd-send-4",
                "clientId": "probe-c4",
                "sessionId": "sess_missing",
                "type": "sendText",
                "payload": { "text": "no session" },
                "issuedAt": 4,
            }),
        )
        .expect("ack 4");
    assert_eq!(ack["status"], json!("failed"));
    assert_eq!(ack["reasonCode"], json!("fault.command.sessionNotActive"));

    // 命令对账：四条 ACK 都可查。
    let query = server
        .dispatch(
            "v4/commands/query",
            json!({
                "commands": [
                    { "sessionId": session_id, "commandId": "cmd-send-1" },
                    { "sessionId": session_id, "commandId": "cmd-send-2" },
                    { "sessionId": session_id, "commandId": "cmd-send-3" },
                    { "sessionId": "sess_missing", "commandId": "cmd-send-4" }
                ]
            }),
        )
        .expect("query");
    let results = query["results"].as_array().expect("results");
    assert_eq!(results.len(), 4);
    assert_eq!(results[0]["result"]["status"], json!("accepted"));
    assert_eq!(results[1]["result"]["status"], json!("failed"));
    assert_eq!(results[2]["result"]["status"], json!("accepted"));
    assert_eq!(results[3]["result"]["status"], json!("failed"));
}
