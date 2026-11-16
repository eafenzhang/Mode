//! SSE（`text/event-stream`）解码：纯函数、不依赖网络。
//!
//! 契约（WHATWG `text/event-stream` 行语法；OpenAI 流式应答只用到其子集）：
//! - 行终止符：`\n`、`\r\n`、`\r` 等价；
//! - `field: value` 行，value 只剥冒号后的**一个**空格；
//! - `:` 开头是注释行（网关心跳常用），对事件内容无贡献；
//! - 空行触发 dispatch：已累积的 `data` 行按 `\n` 拼成一条载荷；
//! - `data` 缓冲为空的空行不产生载荷（协议规定不 dispatch 空事件）。
//!
//! JSON 合法性与 `[DONE]` 哨兵不是本层的职责——它们是 OpenAI Chat Completions 层的
//! 契约（见 [`crate::openai_chat`]）。本层只保证把字节流无损地切成载荷：chunk 在任意
//! 字节处切分都安全（UTF-8 多字节字符被拆到两个 chunk 也一样，因为只有凑齐整行才解码）。

/// 一行 SSE 文本的分类结果（[`parse_sse_line`] 的产出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseLine {
    /// `field: value`；value 已剥掉冒号后的单个空格。
    Field { field: String, value: String },
    /// 空行：事件边界（decoder 在此 dispatch 累积的 data）。
    Blank,
    /// `:` 开头的注释行：对事件内容无贡献。
    Comment,
}

/// 解析一行已按行终止符切好的 SSE 文本。
///
/// 没有 `:` 的行按「field 为整行、value 为空」处理（WHATWG 规定）；本适配器只认
/// `data` 字段，其它 field 与该形态没有可观察差异。
pub fn parse_sse_line(line: &str) -> SseLine {
    if line.is_empty() {
        return SseLine::Blank;
    }
    if line.starts_with(':') {
        return SseLine::Comment;
    }
    let (field, value) = match line.split_once(':') {
        Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
        None => (line, ""),
    };
    SseLine::Field {
        field: field.to_string(),
        value: value.to_string(),
    }
}

/// 把任意切分的字节块解码成 SSE 事件的 `data` 载荷。
///
/// 用法：`bytes_stream` 的每个 chunk 交给 [`SseDecoder::push`]，返回本次凑齐并 dispatch
/// 的载荷（0..n 条）；流结束时调用 [`SseDecoder::finish`]，把没有以空行收尾的最后一个
/// 事件也补出来——严格 WHATWG 语义会在 EOF 丢弃未 dispatch 的事件， OpenAI 的 usage
/// 尾帧就会被静默吞掉；补发之后，正常帧照常消费，被截断的坏帧由上层报出明确的
/// JSON 解析错误，而不是凭空消失。
#[derive(Debug, Default)]
pub struct SseDecoder {
    /// 未凑齐整行的字节。行终止符是字节级标记，按行解码天然避开 UTF-8 拆字问题。
    buffer: Vec<u8>,
    /// 当前事件累积的 data 行（dispatch 时按 `\n` 拼接）。
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段字节（任意切分）；返回本次凑齐并 dispatch 的 `data` 载荷。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(line) = self.next_line() {
            if let Some(payload) = self.consume_line(&line) {
                payloads.push(payload);
            }
        }
        payloads
    }

    /// 流结束时调用：把缓冲里没有行终止符的残行当作最后一行消费（连接在行中间被截断时，
    /// 残行要么是完整载荷、要么让上层报出明确的解析错误，总之不会凭空消失），再补发
    /// 未以空行收尾的尾部事件；之后 decoder 可丢弃。
    pub fn finish(&mut self) -> Vec<String> {
        let mut payloads = Vec::new();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&self.buffer).into_owned();
            self.buffer.clear();
            if let Some(payload) = self.consume_line(&line) {
                payloads.push(payload);
            }
        }
        if let Some(payload) = self.dispatch() {
            payloads.push(payload);
        }
        payloads
    }

    /// 消费一行：`data` 行累积进当前事件，空行触发 dispatch，其余忽略。
    fn consume_line(&mut self, line: &str) -> Option<String> {
        match parse_sse_line(line) {
            SseLine::Field { field, value } if field == "data" => {
                self.data.push(value);
                None
            }
            SseLine::Field { .. } | SseLine::Comment => None,
            SseLine::Blank => self.dispatch(),
        }
    }

    /// dispatch 当前事件：`data` 缓冲为空时按协议不产生载荷（连续空行安静通过）。
    fn dispatch(&mut self) -> Option<String> {
        if self.data.is_empty() {
            return None;
        }
        let payload = self.data.join("\n");
        self.data.clear();
        Some(payload)
    }

    /// 取出缓冲里的下一整行（含终止符处理）；缓冲没有完整行时返回 `None`。
    fn next_line(&mut self) -> Option<String> {
        let position = self
            .buffer
            .iter()
            .position(|byte| *byte == b'\r' || *byte == b'\n')?;
        let line = self.buffer.drain(..position).collect::<Vec<u8>>();
        let terminator = self.buffer.remove(0);
        if terminator == b'\r' && self.buffer.first() == Some(&b'\n') {
            self.buffer.remove(0); // \r\n 作为同一个终止符一并吃掉
        }
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}
