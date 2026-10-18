//! host 层 `CommandInbox`：同 key 的已接受输入串行准入（M2 Task 4 / DR-T4-7 (c)）。
//!
//! **这不是 oracle 观察值**：stdio 会话面对并发 prompt 的行为是**拒绝**
//! （`-32010` / `A prompt is already running for this session`），不排队。队列属于 host
//! 输入面（AGENTS.md：「已接受的 busy/running 输入由 CLI/runtime `CommandInbox` 串行准入」），
//! 跨层对拍等 host 面可观察后再补。规格见 `docs/specs/tauri-agent-runtime.md` §3.4.1。
//!
//! 顺序语义：同 key FIFO；第一个 `admit` 立即成为 running，其余进入队尾。`complete` 只接受
//! 当前 running 的 id——stale 完成（例如旧请求的迟到回调）不得让排队项插队。**没有超时**：
//! 准入顺序由调用方同步推进，禁止用超时掩盖同步问题。

use std::collections::{HashMap, VecDeque};

/// `admit` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// 该 key 当前空闲，命令立即开始。
    Started,
    /// 该 key 已有命令在跑；`position` 从 1 起，按到达顺序。
    Queued { position: usize },
}

/// `complete` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// 完成了当前命令，队首被提升为新的 running。
    Promoted(String),
    /// 完成了当前命令，队列已空。
    Idle,
    /// id 不是当前 running（迟到/重复的完成）；**队列不变**。
    Stale,
}

/// 按 key 维护 running 与 FIFO 队列。
///
/// 调用方负责选择 key（本地/远程的会话粒度由 Host 决定）；本结构不做 key 解析，
/// 也不持有命令内容。
#[derive(Debug, Default)]
pub struct CommandInbox {
    running: HashMap<String, String>,
    queues: HashMap<String, VecDeque<String>>,
}

impl CommandInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// 尝试准入一条命令。首次调用立即开始，后续进入同 key 的队尾。
    pub fn admit(&mut self, key: &str, command_id: impl Into<String>) -> Admission {
        let command_id = command_id.into();
        if self.running.contains_key(key) {
            let queue = self.queues.entry(key.to_string()).or_default();
            queue.push_back(command_id);
            return Admission::Queued {
                position: queue.len(),
            };
        }
        self.running.insert(key.to_string(), command_id);
        Admission::Started
    }

    /// 完成当前 running 的命令；成功时提升队首（如果有）。
    ///
    /// id 不匹配时返回 [`Completion::Stale`]，且**不改变**队列——这条保证让迟到的完成回调
    /// 不会把别的命令当成已结束。
    pub fn complete(&mut self, key: &str, command_id: &str) -> Completion {
        match self.running.get(key) {
            Some(running) if running == command_id => {
                let next = self.queues.get_mut(key).and_then(VecDeque::pop_front);
                match next {
                    Some(next) => {
                        self.running.insert(key.to_string(), next.clone());
                        Completion::Promoted(next)
                    }
                    None => {
                        self.running.remove(key);
                        self.queues.remove(key);
                        Completion::Idle
                    }
                }
            }
            _ => Completion::Stale,
        }
    }

    /// 取消一个**排队中**的命令；running 的取消走 [`CommandInbox::complete`]（终态收口）。
    pub fn cancel_queued(&mut self, key: &str, command_id: &str) -> bool {
        let Some(queue) = self.queues.get_mut(key) else {
            return false;
        };
        let Some(position) = queue.iter().position(|id| id == command_id) else {
            return false;
        };
        queue.remove(position);
        if queue.is_empty() {
            self.queues.remove(key);
        }
        true
    }

    /// 当前 running 的命令 id。
    pub fn running(&self, key: &str) -> Option<&str> {
        self.running.get(key).map(String::as_str)
    }

    /// 当前排队中的命令 id（FIFO 顺序）。
    pub fn pending(&self, key: &str) -> Vec<&str> {
        self.queues
            .get(key)
            .map(|queue| queue.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }
}
