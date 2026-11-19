//! 出站构造：`/sendmessage` 体 + markdown 降级 + CRLF 归一
//! （TS `weixinProvider.ts:593-605,815-836`、`botText.ts:154-172` 的对齐）。

use serde_json::{json, Map, Value};

use crate::headers::{WEIXIN_MESSAGE_STATE_FINISH, WEIXIN_MESSAGE_TYPE_BOT};

/// `client_id`：`mode-weixin-<uuid>`（TS :593-595）。
pub fn build_client_id() -> String {
    format!("mode-weixin-{}", uuid::Uuid::new_v4())
}

/// `/sendmessage` 的 `msg` 体（TS :815-836）。
///
/// `from_user_id`：机器人自身 id——迁移后读 `providerSelfId`，存量配置回退
/// `providerUserId`（spec weixin-provider-self-id）。`context_token` 仅在有值时下发
/// （TS :827 的条件展开）。
pub fn build_send_message(
    from_user_id: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    text: &str,
) -> Value {
    let mut msg = Map::from_iter([
        ("from_user_id".to_string(), json!(from_user_id)),
        ("to_user_id".to_string(), json!(to_user_id)),
        ("client_id".to_string(), json!(build_client_id())),
        ("message_type".to_string(), json!(WEIXIN_MESSAGE_TYPE_BOT)),
        (
            "message_state".to_string(),
            json!(WEIXIN_MESSAGE_STATE_FINISH),
        ),
        (
            "item_list".to_string(),
            json!([{ "type": 1, "text_item": { "text": outbound_text(text) } }]),
        ),
    ]);
    if let Some(token) = context_token {
        msg.insert("context_token".to_string(), json!(token));
    }
    json!({ "msg": Value::Object(msg) })
}

/// 出站文本：markdown 降级 + LF→CRLF（TS :597-605）。
pub fn outbound_text(text: &str) -> String {
    plain_text_reply(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

const CODE_FENCE_LINE: &str = r"(?m)^[ \t]*```[^\n]*$";
const INLINE_CODE: &str = r"`([^`\n]+)`";
const BOLD_ASTERISK: &str = r#"(^|[\s，。；：、！？,;:!?)（(\[【"'—-])\*\*([^*\n]+)\*\*"#;
const BOLD_UNDERSCORE: &str = r#"(^|[\s，。；：、！？,;:!?)（(\[【"'—-])__([^_\n]+)__"#;
const HEADING_MARKER: &str = r"(?m)^[ \t]{0,3}#{1,6}[ \t]+";
const MARKDOWN_LINK: &str = r"\[([^\]\n]*)\]\(([^)\s]+)\)";

/// markdown 降级（TS `botText.ts:154-172`；docs/specs/bot-reply-text-format.md）。
///
/// 只剥离我们主动产生的标记：代码围栏行整行去掉、行内代码与粗体保文字、
/// 标题标记去掉、链接转 `text (url)`；单个星号/下划线（glob、文件名）一律不碰；
/// 幂等：降级后的文本再降级不变。
pub fn plain_text_reply(text: &str) -> String {
    let strip_fence = regex::Regex::new(CODE_FENCE_LINE).expect("合法正则");
    let inline_code = regex::Regex::new(INLINE_CODE).expect("合法正则");
    let bold_asterisk = regex::Regex::new(BOLD_ASTERISK).expect("合法正则");
    let bold_underscore = regex::Regex::new(BOLD_UNDERSCORE).expect("合法正则");
    let heading = regex::Regex::new(HEADING_MARKER).expect("合法正则");
    let link = regex::Regex::new(MARKDOWN_LINK).expect("合法正则");
    let mut out = text.to_string();
    out = strip_fence.replace_all(&out, "").into_owned();
    out = inline_code.replace_all(&out, "$1").into_owned();
    out = bold_asterisk.replace_all(&out, "$1$2").into_owned();
    out = bold_underscore.replace_all(&out, "$1$2").into_owned();
    out = heading.replace_all(&out, "").into_owned();
    out = link.replace_all(&out, "$1 ($2)").into_owned();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_message_body_matches_ts_shape() {
        let body = build_send_message("self-1", "user-1", Some("ctx"), "**帮助**");
        let msg = &body["msg"];
        assert_eq!(msg["from_user_id"], json!("self-1"));
        assert_eq!(msg["to_user_id"], json!("user-1"));
        assert_eq!(msg["message_type"], json!(WEIXIN_MESSAGE_TYPE_BOT));
        assert_eq!(msg["message_state"], json!(WEIXIN_MESSAGE_STATE_FINISH));
        assert_eq!(msg["context_token"], json!("ctx"));
        assert!(msg["client_id"]
            .as_str()
            .expect("client_id")
            .starts_with("mode-weixin-"));
        let item = &msg["item_list"][0];
        assert_eq!(item["type"], json!(1));
        // markdown 降级 + CRLF。
        assert_eq!(item["text_item"]["text"], json!("帮助"));
    }

    #[test]
    fn context_token_omitted_when_absent() {
        let body = build_send_message("self", "user", None, "hi");
        assert!(body["msg"].get("context_token").is_none());
    }

    #[test]
    fn plain_text_reply_strips_only_our_markers() {
        // TS botText.ts 的语义锚点：粗体/行内代码/标题/链接降级；单星号与文件名不碰。
        assert_eq!(plain_text_reply("**/帮助**"), "/帮助");
        assert_eq!(plain_text_reply("`src/a.ts`"), "src/a.ts");
        assert_eq!(plain_text_reply("#### 标题"), "标题");
        assert_eq!(
            plain_text_reply("[文档](https://x/y)"),
            "文档 (https://x/y)"
        );
        assert_eq!(plain_text_reply("src/*.ts"), "src/*.ts");
        assert_eq!(plain_text_reply("my_file_name.md"), "my_file_name.md");
        // 幂等。
        let once = plain_text_reply("**粗** 与 `代码`");
        assert_eq!(plain_text_reply(&once), once);
    }

    #[test]
    fn outbound_text_normalizes_newlines_to_crlf() {
        assert_eq!(outbound_text("a\nb"), "a\r\nb");
        assert_eq!(outbound_text("a\r\nb\rc"), "a\r\nb\r\nc");
    }
}
