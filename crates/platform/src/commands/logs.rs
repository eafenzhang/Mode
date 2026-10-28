//! 日志导出面：`exportLogs`（spec `tauri-platform-commands.md` 命令表第 15 行；LR-5 §27）。
//!
//! 日志由 Host sidecar 写（`<数据基目录>/.mode/v2/logs/host-YYYY-MM-DD.log`，7 日滚动，
//! 见 `packages/desktop/src/host-standalone/logFile.ts`）。本命令只做三件事：
//! 「按 Host 同口径找目录 → 选 Host 线 `host-*.log` 打成一个文本包 → 系统保存对话框落盘」。
//! 旧 Electron 线的日期日志只列入包头的 `omitted` manifest，不默认打包（单份可达 231 MB）。
//! 壳不持有日志真源；默认根 `~/.mode/v2/setting.json` 是 Host 与壳共知的引导文件
//! （`docs/specs/tauri-renderer-host.md` §23），这里只读它的 `dataBaseDir` 指针。
//!
//! 刻意不引压缩依赖：单文件文本包可直接阅读、三平台行为一致（格式见 spec §1.1）。
//! 时间戳用自带的 UTC 格式化（std 无本地时区，也不为一个诊断按钮新增时间库依赖），
//! 文件名与 `exportedAt` 都是 UTC，口径在 spec §1.1 写明。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::ExportLogsResult;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use super::dialog::{await_pick, path_string};

/// 数据基目录解析（与 Host 的 `resolveDataRoot` + `withPersistedDataBaseDir` 同口径）：
/// `MODE_DATA_BASE_DIR`（trim 后非空）→ 默认根指针 `dataBaseDir` → 家目录。
///
/// 壳是 Host 的父进程：`MODE_DATA_BASE_DIR` 由壳继承给 Host，指针读的是同一个文件，
/// 所以两边的解析结果一致——这是导出能对准 Host 日志目录的前提。
pub fn resolve_data_base_dir(
    home: &Path,
    env_override: Option<&str>,
    settings_raw: Option<&str>,
) -> PathBuf {
    if let Some(dir) = env_override.map(str::trim).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(pointer) =
        settings_raw.and_then(sidecar_host::settings_probe::persisted_data_base_dir_from_settings)
    {
        return PathBuf::from(pointer);
    }
    home.to_path_buf()
}

/// 日志目录：`<数据基目录>/.mode/v2/logs`（与 Host 的 `hostLogDir` 同一位置）。
pub fn log_dir_for_data_base_dir(base: &Path) -> PathBuf {
    base.join(".mode").join("v2").join("logs")
}

/// 收集目录下全部 `*.log`，按名称升序。
///
/// 单个文件读失败就跳过：这是诊断导出，不为一个坏文件让整次导出失败；
/// 目录不存在/不可读返回空，由调用方按 `no_logs` 拒绝（不导出空文件冒充成功）。
/// 只认扩展名（大小写不敏感）——旧 Electron 线的日期文件与 Host 的 `host-*.log` 都会带上。
pub fn collect_log_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, Vec<u8>)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_log = Path::new(&name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
            if !is_log {
                return None;
            }
            std::fs::read(entry.path()).ok().map(|bytes| (name, bytes))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// 追加一行到任意日志文件（壳诊断用；自动创建父目录）。
///
/// 与 Host 文件日志分开命名（例如 `host-shell.log`），避免和 Host 的跨日滚动写者抢同一文件；
/// 仍是 `host-*.log` 前缀，`exportLogs` 会一并导出。失败只返回 IO 错误，调用方自行忽略。
pub fn append_log_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

/// Host 线日志文件名：`host-YYYY-MM-DD.log`（大小写不敏感的前后缀）。
fn is_host_log_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("host-") && lower.ends_with(".log")
}

/// 按日滚动的 Host 日志：`host-YYYY-MM-DD.log`。`host-shell.log` 这类壳诊断日志
/// 同样属于 Host 线，但不受 7 日名额限制（否则会挤掉一天的真实 Host 日志）。
fn is_dated_host_log_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != "host-YYYY-MM-DD.log".len() {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    let lower = lower.as_bytes();
    lower[..5].eq_ignore_ascii_case(b"host-")
        && lower[5..9].iter().all(u8::is_ascii_digit)
        && lower[9] == b'-'
        && lower[10..12].iter().all(u8::is_ascii_digit)
        && lower[12] == b'-'
        && lower[13..15].iter().all(u8::is_ascii_digit)
        && lower[15..].eq_ignore_ascii_case(b".log")
}

/// 导出选取结果：`omitted` 保留文件名与字节数，写进包头 manifest，避免静默丢弃。
pub struct LogSelection {
    pub included: Vec<(String, Vec<u8>)>,
    pub omitted: Vec<(String, usize)>,
}

/// 只选 Host 线；其中的日滚动日志按名称保留最新 7 份（Host 自身保留 7 天，
/// 这里兜底清理滞后）。`host-shell.log` 等非日滚动诊断日志不清除，也不占用 7 份名额。
/// 非 Host 的旧线日志全部列入 `omitted`——它们不在当前排障链路上，且单份可能非常大。
pub fn select_log_files(files: Vec<(String, Vec<u8>)>) -> LogSelection {
    let (host, others): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|(name, _)| is_host_log_name(name));
    let (mut dated, non_dated): (Vec<_>, Vec<_>) = host
        .into_iter()
        .partition(|(name, _)| is_dated_host_log_name(name));
    dated.sort_by(|a, b| a.0.cmp(&b.0));

    let mut included = non_dated;
    let mut omitted: Vec<(String, usize)> = others
        .into_iter()
        .map(|(name, bytes)| (name, bytes.len()))
        .collect();
    if dated.len() > 7 {
        let split_at = dated.len() - 7;
        omitted.extend(
            dated
                .drain(..split_at)
                .map(|(name, bytes)| (name, bytes.len())),
        );
    }
    included.extend(dated);
    included.sort_by(|a, b| a.0.cmp(&b.0));
    omitted.sort_by(|a, b| a.0.cmp(&b.0));
    LogSelection { included, omitted }
}

/// 渲染文本包（格式钉在 spec §1.1）：头 + `omitted` manifest + 每段 `===== 文件名 (N bytes) =====`。
///
/// 行尾统一 `\n`；非 UTF-8 字节按 lossy 替换（日志里有混合编码时不因一次替换失败丢整段）。
pub fn render_bundle(
    exported_at: &str,
    log_dir: &Path,
    files: &[(String, Vec<u8>)],
    omitted: &[(String, usize)],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("Mode logs export\n");
    let _ = writeln!(out, "exportedAt: {exported_at}");
    let _ = writeln!(out, "logDir: {}", log_dir.display());
    let _ = writeln!(out, "files: {}", files.len());
    let _ = writeln!(out, "omitted: {}", omitted.len());
    for (name, size) in omitted {
        let _ = writeln!(out, "omittedFile: {name} ({size} bytes)");
    }
    for (name, bytes) in files {
        let _ = write!(out, "\n===== {name} ({} bytes) =====\n", bytes.len());
        out.push_str(&String::from_utf8_lossy(bytes));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// `Mode-logs-YYYYMMDD-HHMMSS.log`（UTC）。
pub fn default_file_name(now: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(now);
    format!("Mode-logs-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.log")
}

/// `YYYY-MM-DDTHH:MM:SSZ`（RFC 3339 的 UTC 形态；秒级精度足够给导出包打点）。
pub fn rfc3339_utc(now: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(now);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `SystemTime` → UTC 年月日时分秒。日期换算用 Howard Hinnant 的 civil_from_days
/// （纯整数运算，无时区数据库）；早于 UNIX_EPOCH 的时间按 1970-01-01 处理（不会发生）。
fn utc_parts(now: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;
    (year, month, day, hour, minute, second)
}

/// 自 1970-01-01 起的天数 → (年, 月, 日)。算法出处同上（日期均匀分布在 400 年周期）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 家目录：`USERPROFILE`（Windows）→ `HOME`（POSIX）。空串视同未设置。
fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|dir| dir.trim().to_string())
                .filter(|dir| !dir.is_empty())
        })
        .map(PathBuf::from)
}

fn failed(code: &str) -> ExportLogsResult {
    ExportLogsResult {
        success: false,
        path: None,
        error: Some(code.to_string()),
    }
}

/// 取消（用户关掉保存对话框）：`success: false`，**不带 error**——取消不是失败。
fn canceled() -> ExportLogsResult {
    ExportLogsResult {
        success: false,
        path: None,
        error: None,
    }
}

/// 导出 Host 文件日志为单个文本包（spec 命令表第 15 行）。
///
/// 顺序：先定位并读日志（无日志直接 `no_logs`，不弹对话框），再让用户选保存位置，
/// 最后写盘。**用户取消不是错误**；失败只回稳定码，路径与 IO 细节进 stderr。
#[tauri::command(rename = "exportLogs")]
pub async fn export_logs<R: tauri::Runtime>(app: AppHandle<R>) -> ExportLogsResult {
    let Some(home) = home_dir() else {
        return failed("home_unavailable");
    };
    let env_override = std::env::var("MODE_DATA_BASE_DIR").ok();
    let settings_raw = std::fs::read_to_string(
        sidecar_host::settings_probe::default_root_settings_file(&home),
    )
    .ok();
    let base = resolve_data_base_dir(&home, env_override.as_deref(), settings_raw.as_deref());
    let log_dir = log_dir_for_data_base_dir(&base);

    let selection = select_log_files(collect_log_files(&log_dir));
    if selection.included.is_empty() {
        eprintln!(
            "[exportLogs] 没有可导出的 Host 日志：{}（omitted {} 个旧线文件）",
            log_dir.display(),
            selection.omitted.len()
        );
        return failed("no_logs");
    }

    let now = SystemTime::now();
    let bundle = render_bundle(&rfc3339_utc(now), &log_dir, &selection.included, &selection.omitted);
    let suggested_name = default_file_name(now);
    let picked = await_pick(move |callback| {
        app.dialog()
            .file()
            .set_file_name(suggested_name)
            .add_filter("Log", &["log", "txt"])
            .save_file(callback);
    })
    .await;

    let Some(path) = path_string(picked) else {
        return canceled();
    };

    match tokio::fs::write(&path, bundle.as_bytes()).await {
        Ok(()) => ExportLogsResult {
            success: true,
            path: Some(path),
            error: None,
        },
        Err(error) => {
            // 细节只进诊断（stderr），上抛稳定码：UI 文案不该被本地路径/IO 细节影响。
            eprintln!("[exportLogs] 写盘失败 {path}：{error}");
            failed("write_failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(seconds)
    }

    #[test]
    fn env_override_wins_over_pointer_and_home() {
        let base = resolve_data_base_dir(
            Path::new("/home/u"),
            Some("  D:/ModeData  "),
            Some(r#"{"dataBaseDir":"E:/Ignored"}"#),
        );
        assert_eq!(base, Path::new("D:/ModeData"));
    }

    #[test]
    fn pointer_applies_only_without_env() {
        let base = resolve_data_base_dir(
            Path::new("/home/u"),
            None,
            Some(r#"{"dataBaseDir":"E:/ModeData"}"#),
        );
        assert_eq!(base, Path::new("E:/ModeData"));
        // 空 env 视同未设置（与 Host 的 firstNonEmpty 同口径）。
        let base = resolve_data_base_dir(
            Path::new("/home/u"),
            Some("   "),
            Some(r#"{"dataBaseDir":"E:/ModeData"}"#),
        );
        assert_eq!(base, Path::new("E:/ModeData"));
    }

    #[test]
    fn broken_or_missing_pointer_falls_back_to_home() {
        for raw in [None, Some(""), Some("{:wq}"), Some("{}")] {
            let base = resolve_data_base_dir(Path::new("/home/u"), None, raw);
            assert_eq!(base, Path::new("/home/u"));
        }
    }

    #[test]
    fn log_dir_matches_host_layout() {
        assert_eq!(
            log_dir_for_data_base_dir(Path::new("/home/u")),
            Path::new("/home/u/.mode/v2/logs")
        );
    }

    #[test]
    fn bundle_has_header_and_sorted_sections() {
        let files = vec![
            ("host-2026-10-25.log".to_string(), b"a\n".to_vec()),
            ("host-2026-10-26.log".to_string(), b"b".to_vec()),
        ];
        let bundle = render_bundle("2026-10-26T01:02:03Z", Path::new("/x/logs"), &files, &[]);
        assert!(bundle.starts_with("Mode logs export\n"));
        assert!(bundle.contains("exportedAt: 2026-10-26T01:02:03Z\n"));
        assert!(bundle.contains("logDir: /x/logs\n"));
        assert!(bundle.contains("files: 2\n"));
        assert!(bundle.contains("omitted: 0\n"));
        assert!(bundle.contains("\n===== host-2026-10-25.log (2 bytes) =====\na\n"));
        // 无尾换行的文件补一个换行，段与段之间不粘连。
        assert!(bundle.contains("\n===== host-2026-10-26.log (1 bytes) =====\nb\n"));
    }

    #[test]
    fn bundle_lists_omitted_files() {
        let files = vec![("host-2026-10-26.log".to_string(), b"a\n".to_vec())];
        let omitted = vec![("2026-10-15.log".to_string(), 231_653_577usize)];
        let bundle = render_bundle("now", Path::new("/x/logs"), &files, &omitted);
        assert!(bundle.contains("files: 1\n"));
        assert!(bundle.contains("omitted: 1\n"));
        assert!(bundle.contains("omittedFile: 2026-10-15.log (231653577 bytes)\n"));
    }

    #[test]
    fn selection_keeps_host_line_and_omits_legacy_logs() {
        let files = vec![
            ("2026-10-15.log".to_string(), b"legacy".to_vec()),
            ("host-2026-10-25.log".to_string(), b"a".to_vec()),
            ("host-2026-10-26.log".to_string(), b"b".to_vec()),
        ];
        let selection = select_log_files(files);
        assert_eq!(
            selection
                .included
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["host-2026-10-25.log", "host-2026-10-26.log"]
        );
        assert_eq!(selection.omitted, vec![("2026-10-15.log".to_string(), 6)]);
    }

    #[test]
    fn selection_keeps_only_newest_seven_host_logs() {
        let mut files = Vec::new();
        for day in 20..=27 {
            files.push((format!("host-2026-10-{day}.log"), b"x".to_vec()));
        }
        let selection = select_log_files(files);
        assert_eq!(selection.included.len(), 7);
        assert_eq!(selection.included[0].0, "host-2026-10-21.log");
        assert_eq!(selection.included[6].0, "host-2026-10-27.log");
        assert_eq!(selection.omitted.len(), 1);
        assert_eq!(selection.omitted[0].0, "host-2026-10-20.log");
    }

    #[test]
    fn selection_keeps_shell_log_outside_the_seven_day_cap() {
        let mut files = Vec::new();
        for day in 20..=27 {
            files.push((format!("host-2026-10-{day}.log"), b"x".to_vec()));
        }
        files.push(("host-shell.log".to_string(), b"shell".to_vec()));

        let selection = select_log_files(files);
        let names: Vec<&str> = selection
            .included
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        // 7 份日日志 + 1 份壳诊断；day 20 被 7 日名额挤掉，shell 不占名额。
        assert_eq!(
            names,
            vec![
                "host-2026-10-21.log",
                "host-2026-10-22.log",
                "host-2026-10-23.log",
                "host-2026-10-24.log",
                "host-2026-10-25.log",
                "host-2026-10-26.log",
                "host-2026-10-27.log",
                "host-shell.log",
            ]
        );
        assert_eq!(selection.omitted, vec![("host-2026-10-20.log".to_string(), 1)]);
    }

    #[test]
    fn dated_host_log_name_rejects_non_date_suffixes() {
        assert!(is_dated_host_log_name("host-2026-10-27.log"));
        assert!(is_dated_host_log_name("HOST-2026-10-27.LOG"));
        assert!(!is_dated_host_log_name("host-shell.log"));
        assert!(!is_dated_host_log_name("host-2026-10-2.log"));
        assert!(!is_dated_host_log_name("host-2026-1a-27.log"));
        assert!(!is_dated_host_log_name("2026-10-27.log"));
    }

    #[test]
    fn selection_without_host_logs_returns_no_included_files() {
        let selection = select_log_files(vec![(
            "2026-10-15.log".to_string(),
            b"legacy".to_vec(),
        )]);
        assert!(selection.included.is_empty());
        assert_eq!(selection.omitted.len(), 1);
    }

    #[test]
    fn append_log_line_creates_parent_and_appends() {
        let dir = std::env::temp_dir().join(format!("mode-shell-log-{}", std::process::id()));
        let path = dir.join("nested").join("host-shell.log");
        let _ = std::fs::remove_dir_all(&dir);

        append_log_line(&path, "[t1] first").expect("first append");
        append_log_line(&path, "[t2] second").expect("second append");

        let content = std::fs::read_to_string(&path).expect("read shell log");
        assert_eq!(content, "[t1] first\n[t2] second\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_tolerates_non_utf8_bytes() {
        let files = vec![("weird.log".to_string(), vec![0xFF, 0xFE, b'o', b'k'])];
        let bundle = render_bundle("now", Path::new("/x"), &files, &[]);
        assert!(bundle.contains("ok"));
    }

    #[test]
    fn timestamps_are_utc() {
        // 2026-10-26T01:02:03Z 的 epoch 秒数：2026-10-26 00:00:00Z = 1792972800。
        let now = at(1_792_976_523);
        assert_eq!(rfc3339_utc(now), "2026-10-26T01:02:03Z");
        assert_eq!(default_file_name(now), "Mode-logs-20261026-010203.log");
    }

    #[test]
    fn epoch_and_leap_boundaries() {
        assert_eq!(rfc3339_utc(at(0)), "1970-01-01T00:00:00Z");
        // 2024-02-29T12:34:56Z = 1709210096。
        assert_eq!(rfc3339_utc(at(1_709_210_096)), "2024-02-29T12:34:56Z");
        // 2000-03-01T00:00:00Z = 951868800（400 年周期内的世纪非闰年边界）。
        assert_eq!(rfc3339_utc(at(951_868_800)), "2000-03-01T00:00:00Z");
    }
}
