//! LAN 配对核心：一次性配对码、长期令牌与已配对客户端记录。
//!
//! 真源：`packages/desktop/src/host/lanAccessServer.ts` 的 `createPairCode` /
//! `consumePairCode` / `redeem` / `verifyAuthToken` / `removeClient` / `resetTokens`
//! 与 `packages/desktop/src/host/lanAccessRecords.ts` 的记录形状。
//!
//! 本 crate 不做 HTTP、WebSocket 或发现；随机源、时钟与凭据存储由宿主注入，
//! 便于对拍与在壳内复用。

use std::collections::HashMap;

use base64::Engine;
use serde::{Deserialize, Serialize};

pub const PAIR_CODE_TTL_MS: u64 = 5 * 60_000;
pub const SERVER_ID_CREDENTIAL_KEY: &str = "lan:access:server-id";
pub const CLIENTS_CREDENTIAL_KEY: &str = "lan:access:clients";
const DEFAULT_CLIENT_LABEL: &str = "LAN client";
const TOUCH_THROTTLE_MS: u64 = 60_000;

/// 已配对客户端记录（令牌本体只存凭据服务，这里只保留展示与撤销所需字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanAccessClientRecord {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub last_used_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_workspace_path: Option<String>,
    /// 只认显式 true；缺键表示未授权（远程桌面逐设备同意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_desktop_authorized: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanAccessPairCode {
    pub code: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanAccessPairResult {
    pub token: String,
    pub server_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 凭据存储端口：生产实现落在 `credential-store`，测试注入内存实现。
pub trait LanCredentialPort {
    fn load(&self, key: &str) -> Result<Option<String>, String>;
    fn save(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

pub fn build_client_token_key(client_id: &str) -> String {
    format!("lan:access:client:{client_id}:token")
}

/// 定长比较；长度不同或为空直接失败（令牌是定长 base64url 串）。
pub fn constant_time_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() || left.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// `randomBytes(3).toString("hex").toUpperCase()` 的等价：宿主给 6 位 hex。
pub fn pair_code_from_hex(hex: &str) -> String {
    hex.trim().to_uppercase()
}

/// `randomBytes(32).toString("base64url")` 的等价。
pub fn mint_client_token(random: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
}

pub struct LanPairingOptions {
    pub machine_name: Option<String>,
    pub random_uuid: Box<dyn Fn() -> String + Send>,
    pub random_hex: Box<dyn Fn() -> String + Send>,
    pub random_token: Box<dyn Fn() -> String + Send>,
    pub now_ms: Box<dyn Fn() -> u64 + Send>,
}

pub struct LanPairing<C: LanCredentialPort> {
    credentials: C,
    options: LanPairingOptions,
    server_id: String,
    clients: Vec<LanAccessClientRecord>,
    tokens: HashMap<String, String>,
    last_touch: HashMap<String, u64>,
    pair_code: Option<LanAccessPairCode>,
}

impl<C: LanCredentialPort> LanPairing<C> {
    /// 读取/初始化安装级 serverId 与已配对记录。
    pub fn load(credentials: C, options: LanPairingOptions) -> Result<Self, String> {
        let mut server_id = credentials
            .load(SERVER_ID_CREDENTIAL_KEY)?
            .unwrap_or_default()
            .trim()
            .to_string();
        if server_id.is_empty() {
            server_id = (options.random_uuid)();
            credentials.save(SERVER_ID_CREDENTIAL_KEY, &server_id)?;
        }
        let clients = parse_clients(credentials.load(CLIENTS_CREDENTIAL_KEY)?);
        let mut tokens = HashMap::new();
        for client in &clients {
            if let Some(token) = credentials.load(&build_client_token_key(&client.id))? {
                let token = token.trim().to_string();
                if !token.is_empty() {
                    tokens.insert(client.id.clone(), token);
                }
            }
        }
        Ok(Self {
            credentials,
            options,
            server_id,
            clients,
            tokens,
            last_touch: HashMap::new(),
            pair_code: None,
        })
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn clients(&self) -> &[LanAccessClientRecord] {
        &self.clients
    }

    pub fn machine_name(&self) -> Option<&str> {
        self.options.machine_name.as_deref()
    }

    /// 生成一次性配对码；旧码直接作废。
    pub fn create_pair_code(&mut self) -> LanAccessPairCode {
        let code = pair_code_from_hex(&(self.options.random_hex)());
        let pair_code = LanAccessPairCode {
            code,
            expires_at: (self.options.now_ms)() + PAIR_CODE_TTL_MS,
        };
        self.pair_code = Some(pair_code.clone());
        pair_code
    }

    /// 当前有效配对码；过期即清除。
    pub fn current_pair_code(&mut self) -> Option<LanAccessPairCode> {
        let now = (self.options.now_ms)();
        if self
            .pair_code
            .as_ref()
            .is_some_and(|code| code.expires_at <= now)
        {
            self.pair_code = None;
        }
        self.pair_code.clone()
    }

    /// 配对码兑换：先消费（无论后续写盘是否成功，码都不能复用），再落客户端与令牌。
    pub fn redeem(
        &mut self,
        code: &str,
        label: Option<&str>,
    ) -> Result<Option<LanAccessPairResult>, String> {
        if !self.consume_pair_code(code) {
            return Ok(None);
        }
        let record = LanAccessClientRecord {
            id: (self.options.random_uuid)(),
            label: label
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(DEFAULT_CLIENT_LABEL)
                .to_string(),
            created_at: (self.options.now_ms)(),
            last_used_at: None,
            last_workspace_path: None,
            remote_desktop_authorized: None,
        };
        let token = (self.options.random_token)();
        self.clients.push(record.clone());
        self.tokens.insert(record.id.clone(), token.clone());
        self.credentials
            .save(&build_client_token_key(&record.id), &token)?;
        self.persist_clients()?;
        Ok(Some(LanAccessPairResult {
            token,
            server_id: self.server_id.clone(),
            name: self.options.machine_name.clone(),
        }))
    }

    /// 令牌校验：命中返回 clientId，并按 60s 节流更新 lastUsedAt。
    pub fn verify_token(&mut self, presented: &str) -> Option<String> {
        let client_id = self
            .tokens
            .iter()
            .find(|(_, token)| constant_time_equal(token, presented))
            .map(|(client_id, _)| client_id.clone())?;
        self.touch_client(&client_id);
        Some(client_id)
    }

    /// 该设备最近打开的工作区；同路径重复上报忽略。
    pub fn record_client_workspace(
        &mut self,
        client_id: &str,
        workspace_path: &str,
    ) -> Result<(), String> {
        let Some(client) = self
            .clients
            .iter_mut()
            .find(|client| client.id == client_id)
        else {
            return Ok(());
        };
        if client.last_workspace_path.as_deref() == Some(workspace_path) {
            return Ok(());
        }
        client.last_workspace_path = Some(workspace_path.to_string());
        self.persist_clients()
    }

    /// 远程桌面逐设备授权；false 时删除字段（缺键 = 未授权）。
    pub fn set_remote_desktop_authorized(
        &mut self,
        client_id: &str,
        authorized: bool,
    ) -> Result<(), String> {
        let Some(client) = self
            .clients
            .iter_mut()
            .find(|client| client.id == client_id)
        else {
            return Ok(());
        };
        client.remote_desktop_authorized = authorized.then_some(true);
        self.persist_clients()
    }

    /// 撤销单个客户端；不存在时为空操作。
    pub fn remove_client(&mut self, client_id: &str) -> Result<(), String> {
        let Some(position) = self
            .clients
            .iter()
            .position(|client| client.id == client_id)
        else {
            return Ok(());
        };
        self.clients.remove(position);
        self.tokens.remove(client_id);
        self.last_touch.remove(client_id);
        self.credentials
            .delete(&build_client_token_key(client_id))?;
        self.persist_clients()
    }

    /// 撤销全部令牌并清空客户端记录。
    pub fn reset_tokens(&mut self) -> Result<(), String> {
        for client in &self.clients {
            self.credentials
                .delete(&build_client_token_key(&client.id))?;
        }
        self.clients.clear();
        self.tokens.clear();
        self.last_touch.clear();
        self.persist_clients()
    }

    fn consume_pair_code(&mut self, input: &str) -> bool {
        let now = (self.options.now_ms)();
        let Some(current) = self.pair_code.as_ref() else {
            return false;
        };
        if current.expires_at <= now {
            self.pair_code = None;
            return false;
        }
        // 错误码不消耗配对码（与 TS 一致）；命中后才一次性作废。
        if !constant_time_equal(&current.code, &pair_code_from_hex(input)) {
            return false;
        }
        self.pair_code = None;
        true
    }

    fn touch_client(&mut self, client_id: &str) {
        let now = (self.options.now_ms)();
        if self
            .last_touch
            .get(client_id)
            .is_some_and(|last| *last > now.saturating_sub(TOUCH_THROTTLE_MS))
        {
            return;
        }
        let Some(client) = self
            .clients
            .iter_mut()
            .find(|client| client.id == client_id)
        else {
            return;
        };
        self.last_touch.insert(client_id.to_string(), now);
        client.last_used_at = Some(now);
        let _ = self.persist_clients();
    }

    fn persist_clients(&self) -> Result<(), String> {
        let text = serde_json::to_string(&self.clients)
            .map_err(|error| format!("serialize lan clients failed: {error}"))?;
        self.credentials.save(CLIENTS_CREDENTIAL_KEY, &text)
    }
}

/// 读取记录数组：非法 JSON/非数组整组按空，单条缺 id 跳过（与 TS 同口径）。
fn parse_clients(raw: Option<String>) -> Vec<LanAccessClientRecord> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&raw) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let object = item.as_object()?;
            let id = object.get("id")?.as_str()?.to_string();
            if id.is_empty() {
                return None;
            }
            let str_field = |key: &str| object.get(key).and_then(|value| value.as_str());
            let u64_field = |key: &str| object.get(key).and_then(|value| value.as_u64());
            Some(LanAccessClientRecord {
                id,
                label: str_field("label").unwrap_or_default().to_string(),
                created_at: u64_field("createdAt").unwrap_or_default(),
                last_used_at: u64_field("lastUsedAt"),
                last_workspace_path: str_field("lastWorkspacePath")
                    .filter(|path| !path.is_empty())
                    .map(str::to_string),
                remote_desktop_authorized: (object
                    .get("remoteDesktopAuthorized")
                    .and_then(|value| value.as_bool())
                    == Some(true))
                .then_some(true),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct MemoryCredentials(RefCell<HashMap<String, String>>);

    impl LanCredentialPort for MemoryCredentials {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.borrow().get(key).cloned())
        }
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(key);
            Ok(())
        }
    }

    fn options() -> LanPairingOptions {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let uuid = {
            let counter = std::sync::Arc::clone(&counter);
            move || {
                let value = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                format!("uuid-{value}")
            }
        };
        let token_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let token = {
            let token_counter = std::sync::Arc::clone(&token_counter);
            move || {
                let value = token_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as u8;
                mint_client_token(&[value; 32])
            }
        };
        LanPairingOptions {
            machine_name: Some("Test Machine".to_string()),
            random_uuid: Box::new(uuid.clone()),
            random_hex: Box::new(|| "a1b2c3".to_string()),
            random_token: Box::new(token),
            now_ms: Box::new(|| 1_000),
        }
    }

    fn pairing(credentials: MemoryCredentials) -> LanPairing<MemoryCredentials> {
        LanPairing::load(credentials, options()).unwrap()
    }

    #[test]
    fn pair_code_is_uppercased_and_single_use() {
        let mut pairing = pairing(MemoryCredentials::default());
        let code = pairing.create_pair_code();
        assert_eq!(code.code, "A1B2C3");
        assert_eq!(code.expires_at, 1_000 + PAIR_CODE_TTL_MS);

        let result = pairing.redeem("a1b2c3", Some("phone")).unwrap().unwrap();
        assert_eq!(result.server_id, "uuid-0");
        assert_eq!(result.name.as_deref(), Some("Test Machine"));
        assert_eq!(pairing.clients().len(), 1);
        assert_eq!(pairing.clients()[0].label, "phone");

        assert!(pairing.redeem("a1b2c3", None).unwrap().is_none());
    }

    #[test]
    fn expired_code_is_rejected() {
        let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(10_000));
        let clock_for_pairing = std::sync::Arc::clone(&clock);
        let mut pairing = LanPairing::load(
            MemoryCredentials::default(),
            LanPairingOptions {
                now_ms: Box::new(move || clock_for_pairing.load(std::sync::atomic::Ordering::SeqCst)),
                ..options()
            },
        )
        .unwrap();
        pairing.create_pair_code();
        clock.store(
            10_000 + PAIR_CODE_TTL_MS + 1,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert!(pairing.redeem("A1B2C3", None).unwrap().is_none());
    }

    #[test]
    fn wrong_code_does_not_consume_the_pair_code() {
        let mut pairing = pairing(MemoryCredentials::default());
        pairing.create_pair_code();
        assert!(pairing.redeem("FFFFFF", None).unwrap().is_none());
        let result = pairing.redeem("A1B2C3", None).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn token_verifies_and_throttles_last_used_writes() {
        let mut pairing = pairing(MemoryCredentials::default());
        pairing.create_pair_code();
        let result = pairing.redeem("A1B2C3", None).unwrap().unwrap();
        assert_eq!(
            pairing.verify_token(&result.token).as_deref(),
            Some("uuid-1")
        );
        assert_eq!(pairing.clients()[0].last_used_at, Some(1_000));
        assert!(pairing.verify_token("wrong-token").is_none());
    }

    #[test]
    fn remove_and_reset_clear_tokens() {
        let mut pairing = pairing(MemoryCredentials::default());
        pairing.create_pair_code();
        let first = pairing.redeem("A1B2C3", None).unwrap().unwrap();
        pairing.create_pair_code();
        let second = pairing.redeem("A1B2C3", None).unwrap().unwrap();

        pairing.remove_client("uuid-1").unwrap();
        assert!(pairing.verify_token(&first.token).is_none());
        assert!(pairing.verify_token(&second.token).is_some());

        pairing.reset_tokens().unwrap();
        assert!(pairing.verify_token(&second.token).is_none());
        assert!(pairing.clients().is_empty());
    }

    #[test]
    fn token_is_base64url_without_padding() {
        let token = mint_client_token(&[0xff; 32]);
        assert!(!token.contains('='));
        assert!(token.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
    }

    #[test]
    fn parses_lenient_client_records() {
        let records = parse_clients(Some(
            r#"[{"id":"a","label":"A","createdAt":1,"lastUsedAt":null,"remoteDesktopAuthorized":true},
                {"label":"missing-id"},{"id":""}]"#
                .to_string(),
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "a");
        assert_eq!(records[0].remote_desktop_authorized, Some(true));
    }
}
