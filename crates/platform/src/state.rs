//! M1 骨架期的托管状态。
//!
//! **所有权声明**：跨窗口 tab 去重、未读数聚合与 renderer-ready 信号的真源仍在既有
//! Node/TS 侧（每个窗口一个 window-scoped Local Host，见 `AGENTS.md`）。M1 只把命令面
//! 接通，这里保存「最后一次同步进来的值」，用于骨架期自测与可观测性；M3 把 sidecar
//! 接上后，这些字段交还给真正的所有者，本结构随之消失。
//!
//! 刻意不在这里做业务判断（例如按 tab 列表算未读），避免出现第二条状态写入路径。
use std::sync::Mutex;

/// M1 命令面托管状态。
#[derive(Default)]
pub struct AppState {
    inner: Mutex<Snapshot>,
}

/// 最近一次由渲染层同步进来的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub tabs: Vec<String>,
    pub unread_count: u32,
    pub renderer_ready: bool,
}

impl AppState {
    /// 覆盖式写入 tab 列表，返回写入后的快照。
    pub fn set_tabs(&self, paths: Vec<String>) -> Snapshot {
        let mut guard = self.lock();
        guard.tabs = paths;
        guard.clone()
    }

    /// 覆盖式写入未读数，返回写入后的快照。
    pub fn set_unread_count(&self, count: u32) -> Snapshot {
        let mut guard = self.lock();
        guard.unread_count = count;
        guard.clone()
    }

    /// 标记 renderer 已就绪，返回写入后的快照。
    pub fn mark_renderer_ready(&self) -> Snapshot {
        let mut guard = self.lock();
        guard.renderer_ready = true;
        guard.clone()
    }

    /// 读取当前快照。
    pub fn snapshot(&self) -> Snapshot {
        self.lock().clone()
    }

    /// 互斥锁只在「读/写一个字段」的临界区持有，且临界区内不做 IO，
    /// 因此中毒（panic during lock）不会留下不一致状态，可直接取回内部值。
    fn lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
