//! 发布给渲染层的 database-startup 状态：**壳是唯一的发布者**（M3 Task 1 的所有权口径）。
//!
//! 渲染层的启动闸门是「`phase === "ready"` 且端口的 `databaseStartupId` 与状态的 `startupId` 同代」，
//! 且在**一条状态都没到**时 30 秒后画失败屏（`renderer/src/main.tsx` 的启动计时器）。
//! 注意那个计时器**一收到任何状态就被拆除**：一旦 `starting` 已经投出去，渲染层就再也没有
//! 自己的超时了——**壳推的 `failed` 是此后唯一的失败信号**（复评 I4(b) 纠正的口径：
//! 「超时是兜底」只在第一帧状态到达之前成立）。所以这条流必须满足两件事：
//!
//! 1. **字段严格等于** `packages/shared/src/database-startup.ts` 的 `databaseStartupStateSchema`
//!    （zod `.strict()`：多一个字段就整条消息被判非法 → 渲染层静默丢弃 → 失败屏）；
//! 2. **失败必须是壳能主动推的**：sidecar 起不来 / 崩了要推 `phase: "failed"` + `errorCode`，
//!    不能指望渲染层自己超时（DR-T1-3：状态一旦开始投递，就只有 `failed` 能告诉它出事了）。
//!
//! 词汇表的跨语言一致性不是靠「记得同步」：`packages/desktop/tests/host-standalone.test.mts`
//! 会扫本文件的 `=> "..."` 字面量，逐个比对 `databaseStartupPhaseSchema` /
//! `databaseStartupErrorCodeSchema` 的真实取值集合；写错一个即红。

use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

/// `databaseStartupStateSchema` 的 `schemaVersion` 字面量。
const SCHEMA_VERSION: u32 = 1;

/// 发布给渲染层的阶段（只列壳**自己**能判断的三档；Host 报来的分段是 [`StartupStage`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPhase {
    /// 已决定拉起 Host，但还没拿到它的第一帧。
    Starting,
    /// Host 的第一帧已到（`ChannelServer` 的 `Initialize`），闸门可以放行。
    Ready,
    /// 起不来 / 崩了 / 超时：渲染层必须看到失败而不是干等。
    Failed,
}

impl StartupPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// **应用启动阶段**的完整取值集合（= `databaseStartupPhaseSchema` 的六个取值）。
///
/// 与 [`StartupPhase`] 分开的理由是所有权：壳发布的是它能判断的粗粒度 `phase`，
/// 而 `failedPhase` 可以带上**阶段精确**的事实——那是 Host 进程内的真事实
/// （`preparing_host_storage` / `starting_services`），壳从 Host 的结构化 stderr 行里读到它
/// （[`crate::host_stderr`]）。两者共用一个词汇表是渲染层 schema 的形态，不是我们的选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupStage {
    Starting,
    PreparingHostStorage,
    PreparingSessionStorage,
    StartingServices,
    Ready,
    Failed,
}

impl StartupStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::PreparingHostStorage => "preparing_host_storage",
            Self::PreparingSessionStorage => "preparing_session_storage",
            Self::StartingServices => "starting_services",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    /// 解析 Host 报来的阶段名。**认不出就返回 `None`**：猜一个阶段比说「不知道」危险，
    /// 渲染层的重试判据（`canRetryDatabaseStartup`）读的就是这个字段。
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "preparing_host_storage" => Some(Self::PreparingHostStorage),
            "preparing_session_storage" => Some(Self::PreparingSessionStorage),
            "starting_services" => Some(Self::StartingServices),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl From<StartupPhase> for StartupStage {
    fn from(phase: StartupPhase) -> Self {
        match phase {
            StartupPhase::Starting => Self::Starting,
            StartupPhase::Ready => Self::Ready,
            StartupPhase::Failed => Self::Failed,
        }
    }
}

/// 失败原因码（取值必须落在 `databaseStartupErrorCodeSchema` 里）。
///
/// 前四个是壳**自己**能判断的（拉不起进程 / 传输断开 / 超时 / 配置非法）；
/// 其余八个是 Host 侧的 `classifyDatabaseStartupError` 可能给出的分类，
/// 壳通过结构化 stderr 行把它们**原样转发**给渲染层（DR-T3-3 ①：失败原因要精确到阶段，
/// 而不是一律塌成「就绪前退出」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupErrorCode {
    /// 拉不起 Host 进程：node / 入口缺失或不可执行（打包形态落点问题会先撞在这里）。
    OpenFailed,
    /// 传输断开：Host 崩了或在就绪前退出。
    TransportClosed,
    /// 就绪期限内没有拿到 Host 的第一帧。
    StartupStatusTimeout,
    /// 运行时不支持（例如入口需要 tsx 而壳侧环境没有它）。
    UnsupportedRuntime,
    /// Host 报来的分类：磁盘满 / 配额。
    StorageFull,
    /// Host 报来的分类：权限。
    PermissionDenied,
    /// Host 报来的分类：IO。
    IoError,
    /// Host 报来的分类：内存。
    OutOfMemory,
    /// Host 报来的分类：库损坏。
    Corrupt,
    /// Host 报来的分类：拿不到锁。
    LockTimeout,
    /// Host 报来的分类：校验和不符。
    ChecksumMismatch,
    /// Host 报来的分类兜底：SQL / 存储打开失败（Host 认不出更具体的分类时用它）。
    SqlFailed,
}

impl StartupErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenFailed => "open_failed",
            Self::TransportClosed => "transport_closed",
            Self::StartupStatusTimeout => "startup_status_timeout",
            Self::UnsupportedRuntime => "unsupported_runtime",
            Self::StorageFull => "storage_full",
            Self::PermissionDenied => "permission_denied",
            Self::IoError => "io_error",
            Self::OutOfMemory => "out_of_memory",
            Self::Corrupt => "corrupt",
            Self::LockTimeout => "lock_timeout",
            Self::ChecksumMismatch => "checksum_mismatch",
            Self::SqlFailed => "sql_failed",
        }
    }

    /// 解析 Host 报来的错误码。**认不出就返回 `None`**（不猜：错码会让渲染层按错的语义画失败屏）。
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "open_failed" => Some(Self::OpenFailed),
            "transport_closed" => Some(Self::TransportClosed),
            "startup_status_timeout" => Some(Self::StartupStatusTimeout),
            "unsupported_runtime" => Some(Self::UnsupportedRuntime),
            "storage_full" => Some(Self::StorageFull),
            "permission_denied" => Some(Self::PermissionDenied),
            "io_error" => Some(Self::IoError),
            "out_of_memory" => Some(Self::OutOfMemory),
            "corrupt" => Some(Self::Corrupt),
            "lock_timeout" => Some(Self::LockTimeout),
            "checksum_mismatch" => Some(Self::ChecksumMismatch),
            "sql_failed" => Some(Self::SqlFailed),
            _ => None,
        }
    }
}

/// 一次启动尝试的不可变快照。字段与 `databaseStartupStateSchema` 一一对应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupSnapshot {
    pub startup_id: String,
    pub attempt_id: String,
    pub sequence: u32,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub phase: StartupPhase,
    pub error_code: Option<StartupErrorCode>,
    pub failed_phase: Option<StartupStage>,
}

impl StartupSnapshot {
    /// 按 zod schema 的**严格**形状序列化：必填字段全在，可选字段只在有值时出现。
    pub fn to_state_json(&self) -> serde_json::Value {
        let mut state = json!({
            "schemaVersion": SCHEMA_VERSION,
            "startupId": self.startup_id,
            "attemptId": self.attempt_id,
            "sequence": self.sequence,
            "startedAt": self.started_at_ms,
            "updatedAt": self.updated_at_ms,
            "phase": self.phase.as_str(),
            // `disk` 恒为空数组：**渲染层不消费它**（本 checkout 里没有任何一处读 `state.disk`），
            // 而它的真产出在 Electron Host 的 `startupDiskSampler`（本包不跑那一层）。
            // Task 3 的处置是**移交**（理由与判据见 spec 的 §15.3 / Task 3 的 DR-T3-3 ③），
            // 不是「忘了填」——schema 要求该字段存在，所以给空数组。
            "disk": [],
        });
        if let Some(error_code) = self.error_code {
            state["errorCode"] = json!(error_code.as_str());
        }
        if let Some(failed_phase) = self.failed_phase {
            state["failedPhase"] = json!(failed_phase.as_str());
        }
        state
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 状态的所有者：壳内部唯一持有者，`sequence` 单调递增，变更时唤醒等待者。
pub struct StartupMonitor {
    snapshot: Mutex<StartupSnapshot>,
    changed: Condvar,
}

impl StartupMonitor {
    /// 建一个 `starting` 快照（`sequence = 0`）。
    pub fn new(startup_id: impl Into<String>, attempt_id: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            snapshot: Mutex::new(StartupSnapshot {
                startup_id: startup_id.into(),
                attempt_id: attempt_id.into(),
                sequence: 0,
                started_at_ms: now,
                updated_at_ms: now,
                phase: StartupPhase::Starting,
                error_code: None,
                failed_phase: None,
            }),
            changed: Condvar::new(),
        }
    }

    /// 发布一个新状态（`sequence + 1`）。**只增不减**：渲染层会按代次丢弃旧状态。
    pub fn publish(
        &self,
        phase: StartupPhase,
        error_code: Option<StartupErrorCode>,
        failed_phase: Option<StartupStage>,
    ) {
        let mut snapshot = match self.snapshot.lock() {
            Ok(guard) => guard,
            // 锁中毒只可能是别的线程 panic；此时状态不可信，但也不能让发布路径二次 panic。
            Err(poisoned) => poisoned.into_inner(),
        };
        snapshot.phase = phase;
        snapshot.error_code = error_code;
        snapshot.failed_phase = failed_phase;
        snapshot.sequence = snapshot.sequence.saturating_add(1);
        snapshot.updated_at_ms = now_ms();
        drop(snapshot);
        self.changed.notify_all();
    }

    pub fn snapshot(&self) -> StartupSnapshot {
        match self.snapshot.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 长轮询：`since` 与当前 `sequence` 不同就立刻返回，否则最多等 `timeout`。
    pub fn wait_for_change(&self, since: Option<u32>, timeout: Duration) -> StartupSnapshot {
        let mut guard = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if since != Some(guard.sequence) {
            return guard.clone();
        }
        let (waited, _) = self
            .changed
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard = waited;
        guard.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_snapshot_is_starting_and_serializes_the_schema_keys() {
        let monitor = StartupMonitor::new("startup-1", "attempt-1");
        let state = monitor.snapshot().to_state_json();
        let object = state.as_object().expect("状态是对象");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["attemptId", "disk", "phase", "schemaVersion", "sequence", "startedAt", "startupId", "updatedAt"],
            "字段集合必须等于 databaseStartupStateSchema 的必填字段（.strict() 多一个即非法）"
        );
        assert_eq!(object["schemaVersion"], json!(1));
        assert_eq!(object["phase"], json!("starting"));
        assert_eq!(object["sequence"], json!(0));
        assert_eq!(object["disk"], json!([]));
        assert!(object.get("errorCode").is_none(), "未失败时不得出现 errorCode");
        assert!(object.get("failedPhase").is_none());
    }

    #[test]
    fn publish_increments_sequence_and_carries_failure_fields() {
        let monitor = StartupMonitor::new("startup-2", "attempt-2");
        monitor.publish(
            StartupPhase::Failed,
            Some(StartupErrorCode::TransportClosed),
            Some(StartupStage::Starting),
        );
        let state = monitor.snapshot().to_state_json();
        assert_eq!(state["sequence"], json!(1));
        assert_eq!(state["phase"], json!("failed"));
        assert_eq!(state["errorCode"], json!("transport_closed"));
        assert_eq!(state["failedPhase"], json!("starting"));
    }

    #[test]
    fn wait_for_change_returns_immediately_when_sequence_moved() {
        let monitor = StartupMonitor::new("startup-3", "attempt-3");
        monitor.publish(StartupPhase::Ready, None, None);
        let snapshot = monitor.wait_for_change(None, Duration::from_millis(50));
        assert_eq!(snapshot.phase, StartupPhase::Ready);
        assert_eq!(snapshot.sequence, 1);
    }

    #[test]
    fn wait_for_change_times_out_without_moving() {
        let monitor = StartupMonitor::new("startup-4", "attempt-4");
        let snapshot = monitor.wait_for_change(Some(0), Duration::from_millis(30));
        assert_eq!(snapshot.sequence, 0, "没有新状态时返回当前快照，而不是挂住");
    }

    #[test]
    fn phase_and_error_vocabulary_matches_the_schema_names() {
        assert_eq!(StartupPhase::Starting.as_str(), "starting");
        assert_eq!(StartupPhase::Ready.as_str(), "ready");
        assert_eq!(StartupPhase::Failed.as_str(), "failed");
        assert_eq!(StartupErrorCode::OpenFailed.as_str(), "open_failed");
        assert_eq!(StartupErrorCode::TransportClosed.as_str(), "transport_closed");
        assert_eq!(StartupErrorCode::StartupStatusTimeout.as_str(), "startup_status_timeout");
        assert_eq!(StartupErrorCode::UnsupportedRuntime.as_str(), "unsupported_runtime");
        // Host 侧分类（DR-T3-3 ① 起壳会把它们原样转发给渲染层）。
        assert_eq!(StartupErrorCode::StorageFull.as_str(), "storage_full");
        assert_eq!(StartupErrorCode::PermissionDenied.as_str(), "permission_denied");
        assert_eq!(StartupErrorCode::IoError.as_str(), "io_error");
        assert_eq!(StartupErrorCode::OutOfMemory.as_str(), "out_of_memory");
        assert_eq!(StartupErrorCode::Corrupt.as_str(), "corrupt");
        assert_eq!(StartupErrorCode::LockTimeout.as_str(), "lock_timeout");
        assert_eq!(StartupErrorCode::ChecksumMismatch.as_str(), "checksum_mismatch");
        assert_eq!(StartupErrorCode::SqlFailed.as_str(), "sql_failed");
        // 阶段词汇表：六个取值一个都不能少（少了就没法把 Host 的分段报给渲染层）。
        for (stage, wire) in [
            (StartupStage::Starting, "starting"),
            (StartupStage::PreparingHostStorage, "preparing_host_storage"),
            (StartupStage::PreparingSessionStorage, "preparing_session_storage"),
            (StartupStage::StartingServices, "starting_services"),
            (StartupStage::Ready, "ready"),
            (StartupStage::Failed, "failed"),
        ] {
            assert_eq!(stage.as_str(), wire);
            assert_eq!(StartupStage::from_wire(wire), Some(stage), "{wire} 必须能解回来");
        }
    }

    #[test]
    fn from_wire_round_trips_every_wire_name_and_rejects_unknown_ones() {
        for code in [
            StartupErrorCode::OpenFailed,
            StartupErrorCode::TransportClosed,
            StartupErrorCode::StartupStatusTimeout,
            StartupErrorCode::UnsupportedRuntime,
            StartupErrorCode::StorageFull,
            StartupErrorCode::PermissionDenied,
            StartupErrorCode::IoError,
            StartupErrorCode::OutOfMemory,
            StartupErrorCode::Corrupt,
            StartupErrorCode::LockTimeout,
            StartupErrorCode::ChecksumMismatch,
            StartupErrorCode::SqlFailed,
        ] {
            assert_eq!(StartupErrorCode::from_wire(code.as_str()), Some(code));
        }
        // 认不出来就说不知道：错码会让渲染层按错的语义画失败屏（`canRetryDatabaseStartup` 读它）。
        assert_eq!(StartupErrorCode::from_wire("?"), None);
        assert_eq!(StartupErrorCode::from_wire("ENOSPC"), None);
        assert_eq!(StartupStage::from_wire("?"), None);
        assert_eq!(StartupStage::from_wire("preparing_host_storage "), None);
    }
}
