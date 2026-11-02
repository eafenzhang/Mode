//! Onboarding 完成记录服务（Rust 实现）。
//!
//! 真源：`packages/services/src/onboarding/onboardingRecordService.ts`。记录是设备级
//! JSON（`<appConfigDir>/onboarding-record.json`），以 deviceMid 为设备锚点，entries
//! 按 userId 覆盖（含 null），decisions 记录跳过/老用户判定；userId 与任务存在性由
//! 调用方注入（TS 侧同样是构造参数）。
//!
//! 规范见 `docs/specs/onboarding-record-service.md`。

mod iso;
mod parse;
mod types;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use atomic_file::{write_text_atomic, WriteOptions};

pub use iso::iso8601_utc;
pub use types::{
    DecisionReason, DecisionStatus, InterfaceMode, Occupation, OnboardingDecision,
    OnboardingPreferencesPatch, OnboardingRecordEntry, OnboardingRecordEntryInput,
    OnboardingRecordFile, OnboardingSettingsSyncPatch, UploadState, RECORD_VERSION,
};

/// 服务依赖：登录 userId（apikey/未登录为 None）与「本机已有任务」判定。
///
/// 返回 `Err` 表示依赖读取失败，与 TS 的 rejected promise 一致：写操作中止、判定上抛。
pub trait OnboardingRecordEnvironment: Send + Sync {
    fn load_user_id(&self) -> Result<Option<String>, String>;
    fn has_existing_local_task(&self) -> Result<bool, String>;
}

pub struct OnboardingRecordService {
    record_path: PathBuf,
    environment: Arc<dyn OnboardingRecordEnvironment>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    write_options: WriteOptions,
    /// 串行化「读-改-写」；TS 用 promise 链达到同一效果。
    write_lock: Mutex<()>,
}

impl OnboardingRecordService {
    pub fn new(
        record_path: impl Into<PathBuf>,
        environment: Arc<dyn OnboardingRecordEnvironment>,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            record_path: record_path.into(),
            environment,
            now_ms,
            write_options: WriteOptions::default(),
            write_lock: Mutex::new(()),
        }
    }

    /// 测试/嵌入式场景可关闭跨进程文件锁。
    pub fn with_write_options(mut self, write_options: WriteOptions) -> Self {
        self.write_options = write_options;
        self
    }

    fn read_file(&self) -> Result<Option<OnboardingRecordFile>, String> {
        match std::fs::read_to_string(&self.record_path) {
            Ok(raw) => Ok(parse::parse_record_file(&raw)),
            // TS 对所有读失败都只 warn 并返回 null（含权限错误）。
            Err(_) => Ok(None),
        }
    }

    fn write_file(&self, file: &OnboardingRecordFile) -> Result<(), String> {
        if let Some(parent) = self.record_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        // 直接序列化结构体而不是 `to_value`：serde_json 默认 Map 是 BTreeMap，
        // 经 Value 中转会把字段写成字典序，偏离 TS `JSON.stringify` 的插入序。
        let text = serde_json::to_string_pretty(file).map_err(|error| error.to_string())?;
        write_text_atomic(&self.record_path, &text, &self.write_options)
            .map_err(|error| error.to_string())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.write_lock
            .lock()
            .map_err(|_| "onboarding record write lock poisoned".to_string())
    }

    fn now_iso(&self) -> String {
        iso8601_utc((self.now_ms)())
    }

    fn load_user_id(&self) -> Result<Option<String>, String> {
        self.environment.load_user_id()
    }

    fn has_existing_local_task(&self) -> Result<bool, String> {
        self.environment.has_existing_local_task()
    }

    /// 追加/覆盖一条完成记录；同一 userId（含 null）至多一条。
    pub fn append_record(
        &self,
        device_mid: &str,
        entry: &OnboardingRecordEntryInput,
    ) -> Result<(), String> {
        let user_id = self.load_user_id()?;
        let _guard = self.lock()?;
        let mut file = match self.read_file()? {
            // deviceMid 以文件内已有值为权威；调用方传入不同值只说明异常。
            Some(file) => file,
            None => OnboardingRecordFile::new(device_mid),
        };
        let record = entry.clone().into_entry(user_id.clone());
        match file.entries.iter().position(|item| item.user_id == user_id) {
            Some(index) => file.entries[index] = record,
            None => file.entries.push(record),
        }
        file.decisions
            .retain(|decision| decision.user_id != user_id);
        self.write_file(&file)
    }

    /// 登录认领：把匿名（null）条目/判定改写为该 userId；已有条目或未登录时空操作。
    pub fn claim_anonymous_record(&self) -> Result<(), String> {
        let Some(user_id) = self.load_user_id()? else {
            return Ok(());
        };
        let _guard = self.lock()?;
        let Some(mut file) = self.read_file()? else {
            return Ok(());
        };
        if file.has_identity_record(Some(&user_id)) {
            return Ok(());
        }
        if let Some(entry) = file
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.user_id.is_none())
        {
            entry.user_id = Some(user_id);
            return self.write_file(&file);
        }
        if let Some(decision) = file
            .decisions
            .iter_mut()
            .rev()
            .find(|decision| decision.user_id.is_none())
        {
            decision.user_id = Some(user_id);
            return self.write_file(&file);
        }
        Ok(())
    }

    /// 是否应触发引导；已有记录/判定为 false，本机已有任务时落 `existing_local_user`。
    pub fn should_onboard(&self, device_mid: &str) -> Result<bool, String> {
        let user_id = self.load_user_id()?;
        if let Some(file) = self.read_file()? {
            if file.has_identity_record(user_id.as_deref()) {
                return Ok(false);
            }
        }
        if !self.has_existing_local_task()? {
            return Ok(true);
        }
        let _guard = self.lock()?;
        let mut file = match self.read_file()? {
            Some(file) => file,
            None => OnboardingRecordFile::new(device_mid),
        };
        if file.has_identity_record(user_id.as_deref()) {
            return Ok(false);
        }
        file.decisions.push(OnboardingDecision {
            user_id,
            status: DecisionStatus::ExistingLocalUser,
            reason: DecisionReason::ExistingLocalTask,
            decided_at: self.now_iso(),
        });
        self.write_file(&file)?;
        Ok(false)
    }

    /// 用户关闭首次引导时持久化 dismissed；已有作答时不写。
    pub fn dismiss_onboarding(&self, device_mid: &str) -> Result<(), String> {
        let user_id = self.load_user_id()?;
        let _guard = self.lock()?;
        let mut file = match self.read_file()? {
            Some(file) => file,
            None => OnboardingRecordFile::new(device_mid),
        };
        if file.entries.iter().any(|entry| entry.user_id == user_id) {
            return Ok(());
        }
        let decision = OnboardingDecision {
            user_id: user_id.clone(),
            status: DecisionStatus::Dismissed,
            reason: DecisionReason::UserClosed,
            decided_at: self.now_iso(),
        };
        match file
            .decisions
            .iter()
            .position(|item| item.user_id == user_id)
        {
            Some(index) => file.decisions[index] = decision,
            None => file.decisions.push(decision),
        }
        self.write_file(&file)
    }

    /// 当前用户最近一条作答；无记录返回 None。
    pub fn get_latest_entry(&self) -> Result<Option<OnboardingRecordEntry>, String> {
        let user_id = self.load_user_id()?;
        let Some(file) = self.read_file()? else {
            return Ok(None);
        };
        Ok(file.latest_entry(user_id.as_deref()).cloned())
    }

    /// 把最近一条作答回填成 settings 补丁；跳过页与未知职业按保守默认处理。
    pub fn sync_settings_from_record(&self) -> Result<Option<OnboardingSettingsSyncPatch>, String> {
        let user_id = self.load_user_id()?;
        let Some(file) = self.read_file()? else {
            return Ok(None);
        };
        let Some(latest) = file.latest_entry(user_id.as_deref()) else {
            return Ok(None);
        };
        let occupation = latest
            .occupation
            .as_deref()
            .and_then(Occupation::parse)
            .unwrap_or(Occupation::Other);
        Ok(Some(OnboardingSettingsSyncPatch {
            onboarding_occupation: occupation,
            proactive_suggestions_enabled: latest.proactive_suggestions_enabled.unwrap_or(false),
            memory_enabled: latest.memory_enabled.unwrap_or(false),
        }))
    }

    /// 手动修改偏好后反向回写 record；当前用户无条目时忽略。
    pub fn update_record_preferences(
        &self,
        patch: &OnboardingPreferencesPatch,
    ) -> Result<(), String> {
        let user_id = self.load_user_id()?;
        let _guard = self.lock()?;
        let Some(mut file) = self.read_file()? else {
            return Ok(());
        };
        let Some(index) = file
            .entries
            .iter()
            .rposition(|entry| entry.user_id == user_id)
        else {
            return Ok(());
        };
        if let Some(value) = patch.memory_enabled {
            file.entries[index].memory_enabled = value;
        }
        if let Some(value) = patch.proactive_suggestions_enabled {
            file.entries[index].proactive_suggestions_enabled = value;
        }
        self.write_file(&file)
    }

    /// 读取整份记录文件（后续上传服务器使用）；不存在或损坏返回 None。
    pub fn get_records(&self) -> Result<Option<OnboardingRecordFile>, String> {
        self.read_file()
    }

    /// 删除记录文件（调试用）。
    pub fn clear_records(&self) -> Result<(), String> {
        let _guard = self.lock()?;
        match std::fs::remove_file(&self.record_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}
