//! 出站 API（spec §6.4/6.5/6.6；SDK :1006-1399 的对齐）：
//! 回复（串行队列）、流式（阻塞/非阻塞）、媒体、主动发送、三步分片上传、下载解密。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::connection::{spawn_connection, ConnectionCallbacks, RunOutcome, WeComConfig};
use crate::download::{decrypt_file, FileDownloader, ReqwestFileDownloader};
use crate::frame::{
    generate_req_id, outbound_frame, Frame, CMD_RESPONSE, CMD_RESPONSE_UPDATE,
    CMD_RESPONSE_WELCOME, CMD_SEND_MSG, CMD_UPLOAD_MEDIA_CHUNK, CMD_UPLOAD_MEDIA_FINISH,
    CMD_UPLOAD_MEDIA_INIT,
};
use crate::reply::{AckResult, ReplyBroker};

/// 上传分片上限（base64 前）：512 KiB（SDK :1175）。
pub const UPLOAD_CHUNK_SIZE: usize = 512 * 1024;
/// 最多 100 片（≈50MB，SDK :1177-1179）。
pub const UPLOAD_MAX_CHUNKS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadResult {
    pub media_type: String,
    pub media_id: String,
    pub created_at: Option<String>,
}

/// 上传计划：分片数与并发档位（SDK :1200）。
pub fn plan_upload(total_size: usize) -> Result<(usize, usize), String> {
    let total_chunks = total_size.div_ceil(UPLOAD_CHUNK_SIZE);
    if total_chunks > UPLOAD_MAX_CHUNKS {
        return Err(format!(
            "File too large: {total_chunks} chunks exceeds maximum of {UPLOAD_MAX_CHUNKS} chunks (max ~50MB)"
        ));
    }
    let concurrency = if total_chunks <= 4 {
        total_chunks.max(1)
    } else if total_chunks <= 10 {
        3
    } else {
        2
    };
    Ok((total_chunks, concurrency))
}

#[derive(Clone)]
pub struct WeComClient {
    config: Arc<WeComConfig>,
    broker: ReplyBroker,
    downloader: Arc<dyn FileDownloader>,
    shutdown: watch::Sender<bool>,
}

impl WeComClient {
    /// 启动连接任务；返回客户端句柄与连接任务句柄。
    pub fn start(
        config: WeComConfig,
        callbacks: ConnectionCallbacks,
    ) -> (Self, tokio::task::JoinHandle<RunOutcome>) {
        Self::start_with_downloader(
            config,
            callbacks,
            Arc::new(ReqwestFileDownloader::new().expect("reqwest downloader")),
        )
    }

    pub fn start_with_downloader(
        config: WeComConfig,
        callbacks: ConnectionCallbacks,
        downloader: Arc<dyn FileDownloader>,
    ) -> (Self, tokio::task::JoinHandle<RunOutcome>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let config = Arc::new(config);
        let (broker, task, _outbound) =
            spawn_connection(Arc::clone(&config), callbacks, shutdown_rx);
        (
            Self {
                config,
                broker,
                downloader,
                shutdown: shutdown_tx,
            },
            task,
        )
    }

    pub fn disconnect(&self) {
        let _ = self.shutdown.send(true);
    }

    /// 上一条同 reqId 是否还在等回执（SDK :1361-1364）。
    pub fn has_pending_reply(&self, frame: &Frame) -> bool {
        self.broker.has_pending(frame.req_id())
    }

    /// 流式文本回复（SDK :1020-1038）。
    pub async fn reply_stream(
        &self,
        req_id: &str,
        stream_id: &str,
        content: &str,
        finish: bool,
        msg_item: Option<Value>,
        feedback: Option<Value>,
    ) -> AckResult {
        let mut stream = json!({
            "id": stream_id,
            "finish": finish,
            "content": content,
        });
        if finish {
            if let Some(items) = msg_item {
                stream["msg_item"] = items;
            }
        }
        if let Some(feedback) = feedback {
            stream["feedback"] = feedback;
        }
        self.enqueue_frame(
            CMD_RESPONSE,
            req_id,
            json!({
                "msgtype": "stream",
                "stream": stream,
            }),
        )
        .await
    }

    /// 非阻塞流式：中间帧在上一条未 ack 时跳过；`finish=true` 必发（SDK :1381-1387）。
    pub async fn reply_stream_non_blocking(
        &self,
        req_id: &str,
        stream_id: &str,
        content: &str,
        finish: bool,
        msg_item: Option<Value>,
        feedback: Option<Value>,
    ) -> Result<Option<Frame>, String> {
        if !finish && self.broker.has_pending(req_id) {
            return Ok(None);
        }
        self.reply_stream(req_id, stream_id, content, finish, msg_item, feedback)
            .await
            .map(Some)
    }

    /// 被动回复媒体（SDK :1281-1294）。
    pub async fn reply_media(
        &self,
        req_id: &str,
        media_type: &str,
        media_id: &str,
        video: Option<(&str, &str)>,
    ) -> AckResult {
        self.enqueue_frame(
            CMD_RESPONSE,
            req_id,
            media_body(media_type, media_id, video),
        )
        .await
    }

    /// 主动发送媒体（SDK :1305-1318）。
    pub async fn send_media_message(
        &self,
        chatid: &str,
        media_type: &str,
        media_id: &str,
        video: Option<(&str, &str)>,
    ) -> AckResult {
        let mut body = media_body(media_type, media_id, video);
        body["chatid"] = json!(chatid);
        let req_id = generate_req_id(CMD_SEND_MSG, now_ms());
        self.enqueue_frame(CMD_SEND_MSG, &req_id, body).await
    }

    /// 主动发送消息（SDK :1153-1160）。
    pub async fn send_message(&self, chatid: &str, body: Value) -> AckResult {
        let mut body = body;
        if let Some(object) = body.as_object_mut() {
            object.insert("chatid".into(), json!(chatid));
        }
        let req_id = generate_req_id(CMD_SEND_MSG, now_ms());
        self.enqueue_frame(CMD_SEND_MSG, &req_id, body).await
    }

    /// 欢迎语回复（SDK :1049-1051）。
    pub async fn reply_welcome(&self, req_id: &str, body: Value) -> AckResult {
        self.enqueue_frame(CMD_RESPONSE_WELCOME, req_id, body).await
    }

    /// 模板卡片更新（SDK :1119-1128）。
    pub async fn update_template_card(
        &self,
        req_id: &str,
        template_card: Value,
        userids: Vec<String>,
    ) -> AckResult {
        let mut body = json!({
            "response_type": "update_template_card",
            "template_card": template_card,
        });
        if !userids.is_empty() {
            body["userids"] = json!(userids);
        }
        self.enqueue_frame(CMD_RESPONSE_UPDATE, req_id, body).await
    }

    /// 三步分片上传（SDK :1171-1269）。
    pub async fn upload_media(
        &self,
        file: Vec<u8>,
        media_type: &str,
        filename: &str,
    ) -> Result<UploadResult, String> {
        let (total_chunks, concurrency) = plan_upload(file.len())?;
        let md5_hex = format!("{:x}", md5::compute(&file));
        let init_req_id = generate_req_id(CMD_UPLOAD_MEDIA_INIT, now_ms());
        let init = self
            .enqueue_frame(
                CMD_UPLOAD_MEDIA_INIT,
                &init_req_id,
                json!({
                    "type": media_type,
                    "filename": filename,
                    "total_size": file.len(),
                    "total_chunks": total_chunks,
                    "md5": md5_hex,
                }),
            )
            .await?;
        let upload_id = init
            .body
            .as_ref()
            .and_then(|body| body.get("upload_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("Upload init failed: no upload_id returned. Response: {init:?}")
            })?
            .to_string();

        let file = Arc::new(file);
        let next_index = Arc::new(AtomicUsize::new(0));
        let errors: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut workers = Vec::new();
        for _ in 0..concurrency.max(1) {
            let file = Arc::clone(&file);
            let next_index = Arc::clone(&next_index);
            let errors = Arc::clone(&errors);
            let upload_id = upload_id.clone();
            let broker = self.broker.clone();
            workers.push(tokio::spawn(async move {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    if index >= total_chunks {
                        return;
                    }
                    let start = index * UPLOAD_CHUNK_SIZE;
                    let end = (start + UPLOAD_CHUNK_SIZE).min(file.len());
                    let data = base64::engine::general_purpose::STANDARD.encode(&file[start..end]);
                    // 单片重试 2 次，退避 500ms×n（SDK :1191-1229）。
                    let mut last_error = String::new();
                    let mut ok = false;
                    for attempt in 0..3u32 {
                        let req_id = generate_req_id(CMD_UPLOAD_MEDIA_CHUNK, now_ms());
                        let frame = outbound_frame(
                            CMD_UPLOAD_MEDIA_CHUNK,
                            &req_id,
                            json!({
                                "upload_id": upload_id,
                                "chunk_index": index,
                                "base64_data": data,
                            }),
                        );
                        let result = broker
                            .enqueue(frame)
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|result| result);
                        match result {
                            Ok(_) => {
                                ok = true;
                                break;
                            }
                            Err(error) => {
                                last_error = error;
                                if attempt < 2 {
                                    tokio::time::sleep(Duration::from_millis(
                                        500 * (attempt as u64 + 1),
                                    ))
                                    .await;
                                }
                            }
                        }
                    }
                    if !ok {
                        errors
                            .lock()
                            .expect("errors")
                            .push(format!("Chunk {index} upload failed: {last_error}"));
                    }
                }
            }));
        }
        for worker in workers {
            let _ = worker.await;
        }
        // 把首错拷出作用域，避免 MutexGuard 跨过后续 await（future 必须 Send）。
        let first_error = {
            let errors = errors.lock().expect("errors");
            errors.first().cloned().map(|first| (first, errors.len()))
        };
        if let Some((first, count)) = first_error {
            return Err(format!(
                "Upload failed: {count} chunk(s) failed. First error: {first}"
            ));
        }

        let finish_req_id = generate_req_id(CMD_UPLOAD_MEDIA_FINISH, now_ms());
        let finish = self
            .enqueue_frame(
                CMD_UPLOAD_MEDIA_FINISH,
                &finish_req_id,
                json!({ "upload_id": upload_id }),
            )
            .await?;
        let body = finish.body.clone().unwrap_or(Value::Null);
        let media_id = body
            .get("media_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("Upload finish failed: no media_id returned. Response: {finish:?}")
            })?
            .to_string();
        Ok(UploadResult {
            media_type: body
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(media_type)
                .to_string(),
            media_id,
            created_at: body
                .get("created_at")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// 下载并（可选）解密文件（SDK :1332-1352）。
    pub async fn download_file(
        &self,
        url: &str,
        aes_key: Option<&str>,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        let (data, filename) = self.downloader.download(url.to_string()).await?;
        match aes_key {
            Some(key) => Ok((decrypt_file(&data, key)?, filename)),
            None => Ok((data, filename)),
        }
    }

    async fn enqueue_frame(&self, cmd: &str, req_id: &str, body: Value) -> AckResult {
        let frame = outbound_frame(cmd, req_id, body);
        self.broker
            .enqueue(frame)
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result)
    }

    pub fn config(&self) -> &WeComConfig {
        &self.config
    }
}

fn media_body(media_type: &str, media_id: &str, video: Option<(&str, &str)>) -> Value {
    let mut content = json!({ "media_id": media_id });
    if media_type == "video" {
        if let Some((title, description)) = video {
            content["title"] = json!(title);
            content["description"] = json!(description);
        }
    }
    json!({ "msgtype": media_type, media_type: content })
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_plan_matches_sdk_concurrency_tiers() {
        assert_eq!(plan_upload(0).unwrap(), (0, 1));
        assert_eq!(plan_upload(100).unwrap(), (1, 1));
        assert_eq!(plan_upload(UPLOAD_CHUNK_SIZE * 4).unwrap(), (4, 4));
        assert_eq!(plan_upload(UPLOAD_CHUNK_SIZE * 5).unwrap(), (5, 3));
        assert_eq!(plan_upload(UPLOAD_CHUNK_SIZE * 10).unwrap(), (10, 3));
        assert_eq!(plan_upload(UPLOAD_CHUNK_SIZE * 11).unwrap(), (11, 2));
        assert!(plan_upload(UPLOAD_CHUNK_SIZE * 101).is_err());
    }

    #[test]
    fn media_body_only_carries_video_options_for_video() {
        let image = media_body("image", "media-1", Some(("t", "d")));
        assert_eq!(image["msgtype"], json!("image"));
        assert_eq!(image["image"], json!({"media_id":"media-1"}));
        let video = media_body("video", "media-1", Some(("t", "d")));
        assert_eq!(
            video["video"],
            json!({"media_id":"media-1","title":"t","description":"d"})
        );
    }
}
