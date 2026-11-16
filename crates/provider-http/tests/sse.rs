//! SSE 解码器的纯函数契约（Task 4 Step 1）：
//! `data:` 多行拼接、`[DONE]` 哨兵透传、空行 dispatch（连续空行不产空载荷）、
//! CRLF / CR 行终止等价、注释与非 data 字段忽略、分块边界（含 UTF-8 字符被拆开）、
//! 非法 JSON 原样透传（JSON 契约归 openai_chat 层）、usage 尾帧与普通事件同块到达、
//! 未以空行收尾的尾部事件由 `finish` 补发。

use provider_http::SseDecoder;

fn push_text(decoder: &mut SseDecoder, text: &str) -> Vec<String> {
    decoder.push(text.as_bytes())
}

#[test]
fn data_lines_in_one_event_join_with_newline() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: first\ndata: second\n\n"),
        vec!["first\nsecond".to_string()]
    );
}

#[test]
fn done_sentinel_passes_through_as_payload() {
    // `[DONE]` 是 OpenAI 层的哨兵；SSE 层只保证它作为一条完整 data 载荷到达。
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: [DONE]\n\n"),
        vec!["[DONE]".to_string()]
    );
}

#[test]
fn blank_line_dispatches_and_consecutive_blanks_emit_nothing() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: x\n\n\n"),
        vec!["x".to_string()]
    );
}

#[test]
fn crlf_and_bare_cr_line_endings_are_equivalent() {
    // \r\n / \r / \n 都是合法行终止符，空行同样可以用 \r\n、\r 或 \n 收尾。
    // `data: b\r\r` 中第二个 \r 终止的是那条空行——恰好一次 dispatch。
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: a\r\n\r\n"),
        vec!["a".to_string()]
    );
    assert_eq!(
        push_text(&mut decoder, "data: b\r\r"),
        vec!["b".to_string()]
    );
    assert_eq!(
        push_text(&mut decoder, "data: c\n\n"),
        vec!["c".to_string()]
    );
}

#[test]
fn comments_and_non_data_fields_are_ignored() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, ": keep-alive\nevent: add\nid: 7\ndata: x\n\n"),
        vec!["x".to_string()]
    );
}

#[test]
fn only_one_leading_space_after_colon_is_stripped() {
    // WHATWG：value 只剥冒号后的一个空格；`data:x` 无空格可剥，`data:  x` 保留第二个。
    let mut decoder = SseDecoder::new();
    assert_eq!(push_text(&mut decoder, "data:x\n\n"), vec!["x".to_string()]);
    assert_eq!(
        push_text(&mut decoder, "data:  x\n\n"),
        vec![" x".to_string()]
    );
}

#[test]
fn byte_by_byte_feeding_and_split_utf8_are_safe() {
    // chunk 切分位置任意（TCP 不保证按行到达）；多字节字符被拆到两个 chunk 也不能坏字。
    let mut decoder = SseDecoder::new();
    let bytes = "data: 你好世界\n\n".as_bytes();
    let mut payloads = Vec::new();
    for byte in bytes {
        payloads.extend(decoder.push(std::slice::from_ref(byte)));
    }
    assert_eq!(payloads, vec!["你好世界".to_string()]);
}

#[test]
fn invalid_json_payload_passes_through_unparsed() {
    // SSE 层只按 text/event-stream 契约切事件；JSON 合法性是 openai_chat 层的契约，
    // 非法载荷必须原样透传，让上层显式报错而不是在解码器里静默丢失。
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: {oops\n\n"),
        vec!["{oops".to_string()]
    );
}

#[test]
fn usage_tail_frame_arriving_in_same_chunk_as_earlier_event() {
    // usage 尾帧常与前面的事件挤在同一个 TCP 分块里；一次 push 可产出多条载荷。
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: {\"a\":1}\n\ndata: {\"usage\":{}}\n\n"),
        vec!["{\"a\":1}".to_string(), "{\"usage\":{}}".to_string()]
    );
}

#[test]
fn finish_flushes_unterminated_trailing_event() {
    // 服务器漏发最后一个空行（或连接被截断在事件中间）时，`finish` 把尾部事件补出来：
    // 补出来的载荷要么是完整 JSON（正常收尾），要么让上层报出明确的解析错误，而不是凭空消失。
    let mut decoder = SseDecoder::new();
    assert_eq!(
        push_text(&mut decoder, "data: a\n\ndata: tail"),
        vec!["a".to_string()]
    );
    assert_eq!(decoder.finish(), vec!["tail".to_string()]);
    assert_eq!(decoder.finish(), Vec::<String>::new());
}
