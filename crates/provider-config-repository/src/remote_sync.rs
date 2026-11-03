//! Mode Built-in Remote Release 刷新控制文件：多进程租约与失败退避。
//!
//! 真源：`packages/provider-node/src/mode-builtin-remote-synchronizer.ts` 的
//! `RefreshControl` / `readControl` / `writeControl` 与 `#refresh` / `#finishLease`
//! 的状态转移。网络 fetch 与 `applyRemoteRelease` 仍在宿主编排层。

use std::io::ErrorKind;
use std::path::Path;

use atomic_file::{acquire_file_lock, write_private_text_atomic, WriteOptions};
use serde_json::{json, Map, Value};

const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshControl {
    pub schema_version: u64,
    pub endpoint_key: String,
    pub lease_id: Option<String>,
    pub lease_until: i64,
    pub next_eligible_at: i64,
    pub failure_count: u64,
}

impl RefreshControl {
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            endpoint_key: String::new(),
            lease_id: None,
            lease_until: 0,
            next_eligible_at: 0,
            failure_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireOutcome {
    Acquired(RefreshControl),
    LeaseHeld,
    NotDue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseFinish {
    Success,
    Failure,
    Cancelled,
}

pub struct AcquireOptions<'a> {
    pub endpoint_key: &'a str,
    pub lease_id: &'a str,
    pub force: bool,
    pub now_ms: i64,
    pub lease_duration_ms: i64,
}

pub struct FinishOptions<'a> {
    pub endpoint_key: &'a str,
    pub lease_id: &'a str,
    pub finish: LeaseFinish,
    pub now_ms: i64,
    pub success_interval_ms: i64,
    pub failure_base_delay_ms: i64,
    pub failure_max_delay_ms: i64,
}

/// 在锁内读取控制文件并尝试取得刷新租约。
pub fn acquire_refresh_lease(
    control_file_path: &Path,
    options: &AcquireOptions<'_>,
) -> Result<AcquireOutcome, String> {
    let endpoint_key = options.endpoint_key.trim();
    if endpoint_key.is_empty() {
        return Err("Mode Built-in 远端 Endpoint 不能为空".to_string());
    }
    ensure_parent(control_file_path)?;
    let _lock = lock(control_file_path)?;
    let current = read_refresh_control(control_file_path)?;
    let same_endpoint = current.endpoint_key == endpoint_key;
    if same_endpoint && current.lease_until > options.now_ms {
        return Ok(AcquireOutcome::LeaseHeld);
    }
    if !options.force && same_endpoint && current.next_eligible_at > options.now_ms {
        return Ok(AcquireOutcome::NotDue);
    }
    let next = RefreshControl {
        schema_version: SCHEMA_VERSION,
        endpoint_key: endpoint_key.to_string(),
        lease_id: Some(options.lease_id.to_string()),
        lease_until: options.now_ms.saturating_add(options.lease_duration_ms),
        next_eligible_at: if same_endpoint {
            current.next_eligible_at
        } else {
            0
        },
        failure_count: if same_endpoint {
            current.failure_count
        } else {
            0
        },
    };
    write_refresh_control(control_file_path, &next)?;
    Ok(AcquireOutcome::Acquired(next))
}

/// 结束租约并推进下次可刷新时间；租约已被他人替换时不写文件，返回 `None`。
pub fn finish_refresh_lease(
    control_file_path: &Path,
    options: &FinishOptions<'_>,
) -> Result<Option<RefreshControl>, String> {
    ensure_parent(control_file_path)?;
    let _lock = lock(control_file_path)?;
    let current = read_refresh_control(control_file_path)?;
    if current.endpoint_key != options.endpoint_key
        || current.lease_id.as_deref() != Some(options.lease_id)
    {
        return Ok(None);
    }
    let (failure_count, delay) = match options.finish {
        LeaseFinish::Cancelled => (current.failure_count, 0),
        LeaseFinish::Success => (0, options.success_interval_ms),
        LeaseFinish::Failure => {
            let failure_count = current.failure_count.saturating_add(1);
            let exponent = failure_count.saturating_sub(1).min(31) as u32;
            let base = options.failure_base_delay_ms.max(1);
            let delay = base
                .saturating_mul(1_i64.checked_shl(exponent).unwrap_or(i64::MAX))
                .min(options.failure_max_delay_ms);
            (failure_count, delay)
        }
    };
    let next = RefreshControl {
        schema_version: SCHEMA_VERSION,
        endpoint_key: current.endpoint_key,
        lease_id: None,
        lease_until: 0,
        next_eligible_at: options.now_ms.saturating_add(delay),
        failure_count,
    };
    write_refresh_control(control_file_path, &next)?;
    Ok(Some(next))
}

/// 读取控制文件；缺失、损坏或 schema 不合法时返回空控制（不阻断刷新）。
pub fn read_refresh_control(path: &Path) -> Result<RefreshControl, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => Ok(parse_control(&value).unwrap_or_else(RefreshControl::empty)),
            Err(_) => Ok(RefreshControl::empty()),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(RefreshControl::empty()),
        Err(error) => Err(error.to_string()),
    }
}

/// 写控制文件；调用方必须已持锁（本函数内部不再抢锁）。
pub fn write_refresh_control(path: &Path, control: &RefreshControl) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(&to_json(control)).map_err(|error| error.to_string())?;
    let options = WriteOptions {
        use_file_lock: false,
        ..WriteOptions::default()
    };
    write_private_text_atomic(path, &text, &options).map_err(|error| error.to_string())
}

fn to_json(control: &RefreshControl) -> Value {
    let mut map = Map::new();
    map.insert("schemaVersion".to_string(), json!(SCHEMA_VERSION));
    map.insert(
        "endpointKey".to_string(),
        json!(control.endpoint_key.clone()),
    );
    if let Some(lease_id) = &control.lease_id {
        map.insert("leaseId".to_string(), json!(lease_id.clone()));
    }
    map.insert("leaseUntil".to_string(), json!(control.lease_until));
    map.insert(
        "nextEligibleAt".to_string(),
        json!(control.next_eligible_at),
    );
    map.insert("failureCount".to_string(), json!(control.failure_count));
    Value::Object(map)
}

fn parse_control(value: &Value) -> Option<RefreshControl> {
    let object = value.as_object()?;
    let schema_version = object.get("schemaVersion")?.as_u64()?;
    if schema_version != SCHEMA_VERSION {
        return None;
    }
    let endpoint_key = object.get("endpointKey")?.as_str()?.to_string();
    let lease_id = match object.get("leaseId") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str()?.to_string()),
    };
    let lease_until = object.get("leaseUntil")?.as_i64()?;
    let next_eligible_at = object.get("nextEligibleAt")?.as_i64()?;
    let failure_count = object.get("failureCount")?.as_u64()?;
    Some(RefreshControl {
        schema_version,
        endpoint_key,
        lease_id,
        lease_until,
        next_eligible_at,
        failure_count,
    })
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())
}

fn lock(path: &Path) -> Result<atomic_file::FileLock, String> {
    acquire_file_lock(path, &WriteOptions::default()).map_err(|error| error.to_string())
}
