//! 记录文件的宽进严出解析：只接受 zod schema 认可的字段形状，损坏即视为「从未记录」。

use serde_json::{Map, Value};

use crate::types::{
    DecisionReason, DecisionStatus, InterfaceMode, OnboardingDecision, OnboardingRecordEntry,
    OnboardingRecordEntryInput, OnboardingRecordFile, UploadState, RECORD_VERSION,
};

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn nullable_string(value: Option<&Value>) -> Option<Option<String>> {
    let value = value?;
    if value.is_null() {
        Some(None)
    } else {
        non_empty_string(Some(value)).map(Some)
    }
}

fn nullable_bool(value: Option<&Value>) -> Option<Option<bool>> {
    let value = value?;
    if value.is_null() {
        Some(None)
    } else {
        value.as_bool().map(Some)
    }
}

fn nullable_interface_mode(value: Option<&Value>) -> Option<Option<InterfaceMode>> {
    let value = value?;
    if value.is_null() {
        Some(None)
    } else {
        match value.as_str()? {
            "coding" => Some(Some(InterfaceMode::Coding)),
            "office" => Some(Some(InterfaceMode::Office)),
            _ => None,
        }
    }
}

fn parse_status(value: Option<&Value>) -> Option<DecisionStatus> {
    match value?.as_str()? {
        "dismissed" => Some(DecisionStatus::Dismissed),
        "existing_local_user" => Some(DecisionStatus::ExistingLocalUser),
        _ => None,
    }
}

fn parse_reason(value: Option<&Value>) -> Option<DecisionReason> {
    match value?.as_str()? {
        "user_closed" => Some(DecisionReason::UserClosed),
        "existing_local_task" => Some(DecisionReason::ExistingLocalTask),
        _ => None,
    }
}

fn parse_entry(value: &Value) -> Option<OnboardingRecordEntry> {
    let object = value.as_object()?;
    let completed_at = non_empty_string(object.get("completedAt"))?;
    if object.get("uploadState")?.as_str()? != "pending" {
        return None;
    }
    Some(OnboardingRecordEntry {
        user_id: nullable_string(object.get("userId"))?,
        occupation: nullable_string(object.get("occupation"))?,
        interface_mode: nullable_interface_mode(object.get("interfaceMode"))?,
        memory_enabled: nullable_bool(object.get("memoryEnabled"))?,
        proactive_suggestions_enabled: nullable_bool(object.get("proactiveSuggestionsEnabled"))?,
        completed_at,
        upload_state: UploadState::Pending,
    })
}

fn parse_decision(value: &Value) -> Option<OnboardingDecision> {
    let object = value.as_object()?;
    Some(OnboardingDecision {
        user_id: nullable_string(object.get("userId"))?,
        status: parse_status(object.get("status"))?,
        reason: parse_reason(object.get("reason"))?,
        decided_at: non_empty_string(object.get("decidedAt"))?,
    })
}

fn parse_entries(value: &Value) -> Option<Vec<OnboardingRecordEntry>> {
    value
        .as_array()?
        .iter()
        .map(parse_entry)
        .collect::<Option<Vec<_>>>()
}

fn parse_decisions(value: &Value) -> Option<Vec<OnboardingDecision>> {
    value
        .as_array()?
        .iter()
        .map(parse_decision)
        .collect::<Option<Vec<_>>>()
}

/// 解析整份记录（v1 读取时补空 decisions 并升级为 v2）。
pub fn parse_record_file(raw: &str) -> Option<OnboardingRecordFile> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    let version = object.get("version")?.as_f64()?;
    if version != 1.0 && version != 2.0 {
        return None;
    }
    let device_mid = non_empty_string(object.get("deviceMid"))?;
    let entries = parse_entries(object.get("entries")?)?;
    let decisions = if version == 2.0 {
        parse_decisions(object.get("decisions")?)?
    } else {
        Vec::new()
    };
    Some(OnboardingRecordFile {
        version: RECORD_VERSION,
        device_mid,
        entries,
        decisions,
    })
}

/// `appendRecord` 入参校验：字段必须存在（可为 null），字符串不能为空。
pub fn parse_entry_input(
    object: &Map<String, Value>,
) -> Result<OnboardingRecordEntryInput, String> {
    let occupation = nullable_string(object.get("occupation"))
        .ok_or_else(|| "occupation must be a non-empty string or null".to_string())?;
    let interface_mode = nullable_interface_mode(object.get("interfaceMode"))
        .ok_or_else(|| "interfaceMode must be coding, office or null".to_string())?;
    let memory_enabled = nullable_bool(object.get("memoryEnabled"))
        .ok_or_else(|| "memoryEnabled must be a boolean or null".to_string())?;
    let proactive_suggestions_enabled = nullable_bool(object.get("proactiveSuggestionsEnabled"))
        .ok_or_else(|| "proactiveSuggestionsEnabled must be a boolean or null".to_string())?;
    let completed_at = non_empty_string(object.get("completedAt"))
        .ok_or_else(|| "completedAt must be a non-empty string".to_string())?;
    Ok(OnboardingRecordEntryInput {
        occupation,
        interface_mode,
        memory_enabled,
        proactive_suggestions_enabled,
        completed_at,
    })
}
