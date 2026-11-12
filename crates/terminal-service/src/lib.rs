//! 终端服务编排：profile 组合 + PTY 会话。
//!
//! 行为契约见 `docs/specs/terminal-service.md`。TS oracle：
//! `packages/services/src/terminal/terminalService.ts`。
//!
//! 边界：
//! - profile 的纯解析在 `terminal-service-resolution`，字体/主题组合在 `terminal-profile`；
//!   本模块只做设置读取、系统探测文件读取与编排。
//! - PTY 进程由 `terminal-session` 唯一持有；本模块不另建会话表。
//! - 事件（data/exit）经 `EventBus` 按会话 id 分发给频道订阅者。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use terminal_profile::{
    compose_terminal_font_profile, detect_alacritty_font_family_from_raw,
    detect_iterm2_profile_from_raw, detect_kitty_font_family_from_raw,
    detect_macos_terminal_profile_from_raw, detect_vs_code_terminal_font_family_from_raw,
    detect_windows_terminal_font_family_from_raw, TerminalDetectedProfile,
};
use terminal_service_resolution::{
    resolve_terminal_cwd_with, resolve_terminal_env_with, resolve_terminal_shell_with,
    resolve_terminal_windows_pty_info_with, windows_pty_info_to_json,
};
use terminal_session::{TerminalCreateResult, TerminalEvent, TerminalSessionManager};

pub type TerminalSettingsSource = Arc<dyn Fn() -> Value + Send + Sync>;
pub type TerminalRawFileReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
pub type TerminalPlistReader = Arc<dyn Fn(&str) -> Option<Value> + Send + Sync>;
pub type TerminalExecutableCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type TerminalDirectoryCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 注入的进程环境；生产用 [`TerminalEnvironment::from_process`]，测试可完全替换。
#[derive(Clone)]
pub struct TerminalEnvironment {
    pub platform: String,
    pub env: IndexMap<String, String>,
    pub os_home: String,
    pub release: String,
    pub settings: TerminalSettingsSource,
    pub raw_file: TerminalRawFileReader,
    pub plist: TerminalPlistReader,
    pub is_executable: TerminalExecutableCheck,
    pub is_usable_directory: TerminalDirectoryCheck,
}

impl TerminalEnvironment {
    pub fn from_process(settings: TerminalSettingsSource) -> Self {
        let env: IndexMap<String, String> = std::env::vars().collect();
        let os_home = env
            .get("HOME")
            .or_else(|| env.get("USERPROFILE"))
            .cloned()
            .unwrap_or_default();
        let env_for_check = env.clone();
        Self {
            platform: process_platform().to_string(),
            release: os_release().to_string(),
            env,
            os_home,
            settings,
            raw_file: Arc::new(|path| std::fs::read_to_string(path).ok()),
            plist: Arc::new(read_plist_json),
            is_executable: Arc::new(move |command| command_is_executable(command, &env_for_check)),
            is_usable_directory: Arc::new(|path| {
                std::fs::metadata(path)
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false)
            }),
        }
    }
}

pub struct TerminalService {
    manager: Arc<TerminalSessionManager>,
    environment: TerminalEnvironment,
    bus: Arc<EventBus>,
}

impl TerminalService {
    pub fn new(environment: TerminalEnvironment) -> Self {
        let bus = Arc::new(EventBus::default());
        let emit_bus = Arc::clone(&bus);
        let manager = Arc::new(TerminalSessionManager::new(Arc::new(move |event| {
            emit_bus.publish(&event);
        })));
        Self {
            manager,
            environment,
            bus,
        }
    }

    /// `terminal.profile`：shell/cwd/字体/主题/Windows PTY。
    pub fn profile(&self, cwd: Option<&str>) -> Result<Value, String> {
        Ok(self.resolve_profile(cwd)?.json)
    }

    /// `terminal.create`：profile + PTY 会话；命中 sessionKey 时返回回放与 reattached。
    pub fn create(
        &self,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        session_key: Option<&str>,
    ) -> Result<Value, String> {
        let profile = self.resolve_profile(cwd)?;
        let env = resolve_terminal_env_with(
            &self.environment.platform,
            &self.environment.env,
            path_delimiter(&self.environment.platform),
        );
        // Windows 的 `=C:` 这类伪环境变量不是合法子进程环境键，过滤掉。
        let env_pairs: Vec<(String, String)> = env
            .into_iter()
            .filter(|(key, _)| !key.is_empty() && !key.contains('='))
            .collect();
        let result = self
            .manager
            .create_with_key_env(
                cols,
                rows,
                Some(&profile.cwd),
                &profile.shell,
                session_key,
                Some(&env_pairs),
            )
            .map_err(|error| {
                format!(
                    "Failed to start terminal with shell '{}' in '{}': {error}",
                    profile.shell, profile.cwd
                )
            })?;
        Ok(create_result_json(&profile.json, result))
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        self.manager
            .write(id, data)
            .map_err(|error| map_not_found(error, id))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        self.manager
            .resize(id, cols, rows)
            .map_err(|error| map_not_found(error, id))
    }

    pub fn dispose(&self, id: &str) -> Result<(), String> {
        self.manager.dispose(id)
    }

    pub fn dispose_by_key(&self, key: &str) -> Result<Option<String>, String> {
        self.manager.dispose_by_key(key)
    }

    pub fn subscribe_data(
        &self,
        id: &str,
        listener: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> TerminalSubscription {
        self.bus.subscribe(id, Listener::Data(listener))
    }

    pub fn subscribe_exit(
        &self,
        id: &str,
        listener: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> TerminalSubscription {
        self.bus.subscribe(id, Listener::Exit(listener))
    }

    fn resolve_profile(&self, cwd: Option<&str>) -> Result<ResolvedProfile, String> {
        let environment = &self.environment;
        let shell =
            resolve_terminal_shell_with(&environment.platform, &environment.env, |value| {
                (environment.is_executable)(value)
            })?;
        let cwd =
            resolve_terminal_cwd_with(cwd, &environment.env, &environment.os_home, |value| {
                (environment.is_usable_directory)(value)
            })?;

        let settings = (environment.settings)();
        let font_family = settings.get("terminalFontFamily").and_then(Value::as_str);
        let inherit_system_profile = settings
            .get("terminalInheritSystemProfile")
            .and_then(Value::as_bool)
            != Some(false);
        let detected = if inherit_system_profile {
            self.detect_system_profile()
        } else {
            None
        };
        let font = compose_terminal_font_profile(font_family, detected.as_ref());

        let mut map = Map::new();
        map.insert("shell".to_string(), json!(shell));
        map.insert("cwd".to_string(), json!(cwd));
        map.insert("fontFamily".to_string(), json!(font.font_family));
        if let Some(font_size) = font.font_size {
            map.insert("fontSize".to_string(), json_number(font_size));
        }
        if let Some(theme) = &font.theme {
            map.insert(
                "theme".to_string(),
                Value::Object(
                    theme
                        .iter()
                        .map(|(key, value)| (key.clone(), json!(value)))
                        .collect(),
                ),
            );
        }
        map.insert("fontFamilySource".to_string(), json!(font.source.as_str()));
        if let Some(info) =
            resolve_terminal_windows_pty_info_with(&environment.platform, &environment.release)
        {
            map.insert("windowsPty".to_string(), windows_pty_info_to_json(&info));
        }

        Ok(ResolvedProfile {
            shell,
            cwd,
            json: Value::Object(map),
        })
    }

    /// 系统终端 profile 探测；顺序与 TS `TERMINAL_FONT_DETECTORS` 一致。
    fn detect_system_profile(&self) -> Option<TerminalDetectedProfile> {
        let environment = &self.environment;
        let env: BTreeMap<String, String> = environment
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let reader = |path: &str| (environment.raw_file)(path);
        let raw_reader: &dyn Fn(&str) -> Option<String> = &reader;
        let platform = environment.platform.as_str();

        if platform == "win32" {
            if let Some(profile) =
                detect_windows_terminal_font_family_from_raw(platform, &env, raw_reader)
            {
                return Some(profile);
            }
        }

        if let Some(profile) = detect_vs_code_terminal_font_family_from_raw(&env, raw_reader) {
            return Some(profile);
        }

        if platform == "darwin" {
            if let Some(profile) = self.detect_macos_plist(
                "com.googlecode.iterm2.plist",
                detect_iterm2_profile_from_raw,
            ) {
                return Some(profile);
            }
            if let Some(profile) = self.detect_macos_plist(
                "com.apple.Terminal.plist",
                detect_macos_terminal_profile_from_raw,
            ) {
                return Some(profile);
            }
        }

        if matches!(platform, "darwin" | "linux" | "freebsd" | "openbsd") {
            if let Some(profile) = detect_kitty_font_family_from_raw(&env, raw_reader) {
                return Some(profile);
            }
            if let Some(profile) = detect_alacritty_font_family_from_raw(&env, raw_reader) {
                return Some(profile);
            }
        }

        None
    }

    fn detect_macos_plist(
        &self,
        file_name: &str,
        detector: fn(&Value) -> Option<TerminalDetectedProfile>,
    ) -> Option<TerminalDetectedProfile> {
        let home = self.macos_home();
        let path = PathBuf::from(home)
            .join("Library")
            .join("Preferences")
            .join(file_name);
        let plist = (self.environment.plist)(&path.to_string_lossy())?;
        detector(&plist)
    }

    fn macos_home(&self) -> String {
        self.environment
            .env
            .get("HOME")
            .or_else(|| self.environment.env.get("USERPROFILE"))
            .cloned()
            .unwrap_or_else(|| self.environment.os_home.clone())
    }
}

struct ResolvedProfile {
    shell: String,
    cwd: String,
    json: Value,
}

fn create_result_json(profile: &Value, result: TerminalCreateResult) -> Value {
    let mut map = match profile {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    map.insert("id".to_string(), json!(result.id));
    if result.reattached {
        map.insert("replay".to_string(), json!(result.replay));
        map.insert("reattached".to_string(), json!(true));
    }
    Value::Object(map)
}

fn map_not_found(error: String, id: &str) -> String {
    if error.contains("not found") {
        format!("Terminal not found: {id}")
    } else {
        error
    }
}

fn json_number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number((value as i64).into())
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn path_delimiter(platform: &str) -> char {
    if platform == "win32" {
        ';'
    } else {
        ':'
    }
}

fn process_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// `os.release()` 的 Windows 等价物：`ver` 输出只取一次，供 `windowsPty.buildNumber` 使用。
fn os_release() -> &'static str {
    static RELEASE: OnceLock<String> = OnceLock::new();
    RELEASE.get_or_init(|| {
        if std::env::consts::OS != "windows" {
            return String::new();
        }
        let output = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .ok();
        let Some(output) = output else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(start) = text.find("Version ") else {
            return String::new();
        };
        text[start + "Version ".len()..]
            .trim()
            .trim_end_matches(']')
            .to_string()
    })
}

fn command_is_executable(command: &str, env: &IndexMap<String, String>) -> bool {
    if command.contains('/') || command.contains('\\') {
        return is_executable_path(Path::new(command));
    }
    let Some(path_env) = env.get("PATH") else {
        return false;
    };
    std::env::split_paths(path_env).any(|dir| is_executable_path(&dir.join(command)))
}

fn is_executable_path(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    // Windows 只看「是不是文件」；Unix 还要可执行位。写成两条独立分支，
    // 避免 `if !is_file { return false } ... true` 触发 clippy::needless_bool（-D warnings 会拦 CI）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

#[cfg(target_os = "macos")]
fn read_plist_json(path: &str) -> Option<Value> {
    let output = std::process::Command::new("plutil")
        .args(["-convert", "json", "-o", "-", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

#[cfg(not(target_os = "macos"))]
fn read_plist_json(_path: &str) -> Option<Value> {
    None
}

#[derive(Clone)]
enum Listener {
    Data(Arc<dyn Fn(&str) + Send + Sync>),
    Exit(Arc<dyn Fn(i64) + Send + Sync>),
}

#[derive(Default)]
struct EventBus {
    next_token: AtomicU64,
    listeners: Mutex<HashMap<u64, (String, Listener)>>,
}

impl EventBus {
    fn publish(&self, event: &TerminalEvent) {
        let id = match event {
            TerminalEvent::Data { id, .. } | TerminalEvent::Exit { id, .. } => id,
        };
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|(listener_id, _)| listener_id == id)
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            match (event, listener) {
                (TerminalEvent::Data { data, .. }, Listener::Data(callback)) => callback(data),
                (TerminalEvent::Exit { code, .. }, Listener::Exit(callback)) => callback(*code),
                _ => {}
            }
        }
    }

    fn subscribe(self: &Arc<Self>, id: &str, listener: Listener) -> TerminalSubscription {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token, (id.to_string(), listener));
        TerminalSubscription {
            bus: Arc::clone(self),
            token,
        }
    }

    fn remove(&self, token: u64) {
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&token);
    }
}

/// 事件订阅句柄；drop 时解除。频道层把它包成 `rpc_host::Subscription`。
pub struct TerminalSubscription {
    bus: Arc<EventBus>,
    token: u64,
}

impl Drop for TerminalSubscription {
    fn drop(&mut self) {
        self.bus.remove(self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn fake_environment(platform: &str) -> TerminalEnvironment {
        let mut env = IndexMap::new();
        env.insert("HOME".to_string(), "/home/test".to_string());
        if platform == "win32" {
            env.insert("ComSpec".to_string(), "cmd.exe".to_string());
        } else {
            env.insert("SHELL".to_string(), "/bin/sh".to_string());
            env.insert("PATH".to_string(), "/bin".to_string());
        }
        TerminalEnvironment {
            platform: platform.to_string(),
            env,
            os_home: "/home/test".to_string(),
            release: "10.0.19045.3803".to_string(),
            settings: Arc::new(|| json!({})),
            raw_file: Arc::new(|_| None),
            plist: Arc::new(|_| None),
            is_executable: Arc::new(|command| {
                command == "/bin/sh" || command == "cmd.exe" || command == "pwsh.exe"
            }),
            is_usable_directory: Arc::new(|_| true),
        }
    }

    #[test]
    fn profile_resolves_shell_cwd_and_fallback_font() {
        let service = TerminalService::new(fake_environment("linux"));
        let profile = service.profile(Some("/workspace")).expect("profile");
        assert_eq!(profile["shell"], json!("/bin/sh"));
        assert_eq!(profile["cwd"], json!("/workspace"));
        assert_eq!(profile["fontFamilySource"], json!("fallback"));
        assert!(profile.get("windowsPty").is_none());
    }

    #[test]
    fn profile_honors_custom_font_and_windows_pty() {
        let mut environment = fake_environment("win32");
        environment.settings = Arc::new(|| json!({ "terminalFontFamily": "Cascadia Mono" }));
        environment.is_executable = Arc::new(|command| command == "cmd.exe");
        let service = TerminalService::new(environment);
        let profile = service.profile(None).expect("profile");
        assert_eq!(profile["shell"], json!("cmd.exe"));
        assert_eq!(profile["fontFamilySource"], json!("custom"));
        assert!(profile["fontFamily"]
            .as_str()
            .unwrap()
            .starts_with("Cascadia Mono, "));
        assert_eq!(profile["windowsPty"]["backend"], json!("conpty"));
        assert_eq!(profile["windowsPty"]["buildNumber"], json!(19045));
    }

    #[test]
    fn missing_shell_reports_ts_error() {
        let mut environment = fake_environment("linux");
        environment.is_executable = Arc::new(|_| false);
        let service = TerminalService::new(environment);
        assert_eq!(
            service.profile(None).unwrap_err(),
            "No usable shell found for terminal startup"
        );
    }

    #[test]
    fn write_unknown_id_uses_ts_error() {
        let service = TerminalService::new(fake_environment("linux"));
        assert_eq!(
            service.write("missing", "x").unwrap_err(),
            "Terminal not found: missing"
        );
    }

    #[test]
    fn create_streams_data_and_exit_events() {
        let service = TerminalService::new(TerminalEnvironment::from_process(Arc::new(
            || json!({ "terminalInheritSystemProfile": false }),
        )));
        let data = Arc::new(Mutex::new(String::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let created = service
            .create(80, 24, None, Some("test:terminal:events"))
            .expect("create");
        let id = created["id"].as_str().expect("id").to_string();
        let data_sink = Arc::clone(&data);
        let exit_sink = Arc::clone(&exited);
        let _data_sub = service.subscribe_data(
            &id,
            Arc::new(move |chunk: &str| {
                data_sink
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push_str(chunk);
            }),
        );
        let _exit_sub = service.subscribe_exit(
            &id,
            Arc::new(move |_| exit_sink.store(true, Ordering::SeqCst)),
        );

        #[cfg(windows)]
        let command = "echo __TERM_SERVICE__\r\nexit\r\n";
        #[cfg(not(windows))]
        let command = "echo __TERM_SERVICE__\nexit\n";
        service.write(&id, command).expect("write");

        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let has_marker = data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains("__TERM_SERVICE__");
            if has_marker && exited.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            data.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains("__TERM_SERVICE__"),
            "输出应包含命令回显"
        );
        assert!(exited.load(Ordering::SeqCst), "退出后应收到 exit 事件");
    }
}
