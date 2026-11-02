//! macOS Terminal / iTerm2 plist 原始探测。
//!
//! 真源：`packages/services/src/terminal/terminalProfileMacOs.ts` 的
//! `detectIterm2ProfileFromRaw` 与 `detectMacOsTerminalProfileFromRaw`。
//! plist 转换由调用方注入；本模块只做纯 JSON 值解析。

use base64::Engine;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Number, Value};

use crate::TerminalDetectedProfile;

fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn normalize_font_family(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_float(value: Option<&Value>) -> Option<f64> {
    let numeric = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) if !text.trim().is_empty() => {
            // 与 TS Number.parseFloat 一致：取前缀数字，忽略后缀。
            let pattern =
                Regex::new(r"^[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:[eE][+-]?\d+)?").expect("浮点正则");
            let captures = pattern.captures(text.trim())?;
            captures[0].parse::<f64>().ok()?
        }
        _ => return None,
    };
    numeric.is_finite().then_some(numeric)
}

fn normalize_font_size(value: Option<&Value>) -> Option<f64> {
    parse_float(value).filter(|value| (6.0..=72.0).contains(value))
}

fn read_nested_string(value: Option<&Value>, path_segments: &[&str]) -> Option<String> {
    let mut current = value?;
    for segment in path_segments {
        current = current.as_object()?.get(*segment)?;
    }
    normalize_font_family(Some(current))
}

fn normalize_macos_font_name(value: Option<&Value>) -> Option<String> {
    let normalized = normalize_font_family(value)?;
    let pattern = Regex::new(r"\s+\d+(?:\.\d+)?$").expect("字号正则可编译");
    let without_size = pattern.replace(&normalized, "");
    normalize_font_family(Some(&Value::String(without_size.replace('-', " "))))
}

fn read_macos_font_descriptor(value: Option<&Value>) -> (Option<String>, Option<f64>) {
    let raw = match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(boolean)) => boolean.to_string(),
        _ => String::new(),
    };
    let font_family = normalize_macos_font_name(Some(&Value::String(raw.clone())));
    let pattern = Regex::new(r"\s+(\d+(?:\.\d+)?)$").expect("字号正则可编译");
    let font_size = pattern
        .captures(&raw)
        .and_then(|captures| normalize_font_size(Some(&Value::String(captures[1].to_string()))));
    (font_family, font_size)
}

fn read_macos_archived_font_name(value: Option<&Value>) -> Option<String> {
    let raw_data = match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(object)) => object
            .get("NS")
            .and_then(Value::as_str)
            .map(str::to_string)?,
        _ => return None,
    };
    let normalized = normalize_font_family(Some(&Value::String(raw_data)))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(normalized.as_bytes())
        .ok()?;
    let latin1 = decoded.iter().map(|byte| *byte as char).collect::<String>();
    let pattern = Regex::new(
        r"(?i)([A-Za-z][A-Za-z0-9 ._-]*(?:Mono|Code|Nerd|Powerline|Console|Menlo|Monaco|Courier|Cascadia|Consolas|Hack|Meslo)[A-Za-z0-9 ._-]*)",
    )
    .expect("归档字体正则可编译");
    normalize_macos_font_name(Some(&Value::String(
        pattern.captures(&latin1)?.get(1)?.as_str().to_string(),
    )))
}

fn normalize_color_component(value: Option<&Value>) -> Option<f64> {
    let parsed = parse_float(value)?;
    if (0.0..=1.0).contains(&parsed) {
        return Some(parsed);
    }
    if (0.0..=255.0).contains(&parsed) {
        return Some(parsed / 255.0);
    }
    if (0.0..=65_535.0).contains(&parsed) {
        return Some(parsed / 65_535.0);
    }
    None
}

fn read_color_record_value<'a>(
    record: &'a serde_json::Map<String, Value>,
    names: &[&str],
) -> Option<&'a Value> {
    names
        .iter()
        .find_map(|name| record.get(*name).filter(|value| !value.is_null()))
}

fn format_alpha(alpha: f64) -> String {
    let rendered = format!("{alpha:.3}");
    let rendered = rendered.trim_end_matches('0');
    rendered.trim_end_matches('.').to_string()
}

fn normalize_macos_color(value: Option<&Value>) -> Option<String> {
    if let Some(text) = value.and_then(Value::as_str) {
        let trimmed = text.trim();
        let hex = Regex::new(r"(?i)^#[0-9a-f]{3}(?:[0-9a-f]{3})?(?:[0-9a-f]{2})?$")
            .expect("hex 颜色正则可编译");
        let lowered = trimmed.to_lowercase();
        if hex.is_match(trimmed) || lowered.starts_with("rgb(") || lowered.starts_with("rgba(") {
            return Some(trimmed.to_string());
        }
        return None;
    }
    let record = value?.as_object()?;
    let red = normalize_color_component(read_color_record_value(
        record,
        &["Red Component", "red", "Red"],
    ));
    let green = normalize_color_component(read_color_record_value(
        record,
        &["Green Component", "green", "Green"],
    ));
    let blue = normalize_color_component(read_color_record_value(
        record,
        &["Blue Component", "blue", "Blue"],
    ));
    let alpha = normalize_color_component(read_color_record_value(
        record,
        &["Alpha Component", "alpha", "Alpha", "Opacity"],
    ))
    .unwrap_or(1.0);
    let (Some(red), Some(green), Some(blue)) = (red, green, blue) else {
        return None;
    };
    let r = (red * 255.0).round() as i64;
    let g = (green * 255.0).round() as i64;
    let b = (blue * 255.0).round() as i64;
    if alpha < 1.0 {
        return Some(format!("rgba({r}, {g}, {b}, {})", format_alpha(alpha)));
    }
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn read_theme_color(
    profile: &serde_json::Map<String, Value>,
    theme: &mut IndexMap<String, String>,
    theme_key: &str,
    profile_keys: &[&str],
) {
    for profile_key in profile_keys {
        if let Some(color) = normalize_macos_color(profile.get(*profile_key)) {
            theme.insert(theme_key.to_string(), color);
            return;
        }
    }
}

const ANSI_THEME_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

fn compact_theme(theme: IndexMap<String, String>) -> Option<IndexMap<String, String>> {
    (!theme.is_empty()).then_some(theme)
}

fn read_iterm2_theme(profile: &serde_json::Map<String, Value>) -> Option<IndexMap<String, String>> {
    let mut theme = IndexMap::new();
    read_theme_color(profile, &mut theme, "foreground", &["Foreground Color"]);
    read_theme_color(profile, &mut theme, "background", &["Background Color"]);
    read_theme_color(profile, &mut theme, "cursor", &["Cursor Color"]);
    read_theme_color(profile, &mut theme, "cursorAccent", &["Cursor Text Color"]);
    read_theme_color(
        profile,
        &mut theme,
        "selectionBackground",
        &["Selection Color"],
    );
    for (index, theme_key) in ANSI_THEME_KEYS.iter().enumerate() {
        read_theme_color(
            profile,
            &mut theme,
            theme_key,
            &[
                &format!("Ansi {index} Color"),
                &format!("ANSI {index} Color"),
            ],
        );
    }
    compact_theme(theme)
}

fn read_macos_terminal_theme(
    profile: &serde_json::Map<String, Value>,
) -> Option<IndexMap<String, String>> {
    let mut theme = IndexMap::new();
    read_theme_color(profile, &mut theme, "foreground", &["TextColor"]);
    read_theme_color(profile, &mut theme, "background", &["BackgroundColor"]);
    read_theme_color(profile, &mut theme, "cursor", &["CursorColor"]);
    read_theme_color(
        profile,
        &mut theme,
        "selectionBackground",
        &["SelectionColor"],
    );
    let terminal_ansi_names = [
        "ANSIBlackColor",
        "ANSIRedColor",
        "ANSIGreenColor",
        "ANSIYellowColor",
        "ANSIBlueColor",
        "ANSIMagentaColor",
        "ANSICyanColor",
        "ANSIWhiteColor",
        "ANSIBrightBlackColor",
        "ANSIBrightRedColor",
        "ANSIBrightGreenColor",
        "ANSIBrightYellowColor",
        "ANSIBrightBlueColor",
        "ANSIBrightMagentaColor",
        "ANSIBrightCyanColor",
        "ANSIBrightWhiteColor",
    ];
    for (index, theme_key) in ANSI_THEME_KEYS.iter().enumerate() {
        if let Some(name) = terminal_ansi_names.get(index) {
            read_theme_color(profile, &mut theme, theme_key, &[name]);
        }
    }
    compact_theme(theme)
}

fn has_detected_profile(profile: &TerminalDetectedProfile) -> bool {
    profile.font_family.is_some() || profile.font_size.is_some() || profile.theme.is_some()
}

pub fn detect_iterm2_profile_from_raw(plist: &Value) -> Option<TerminalDetectedProfile> {
    let plist = plist.as_object()?;
    let profiles = plist.get("New Bookmarks").and_then(Value::as_array)?;
    let default_index = profiles.iter().position(|profile| {
        profile
            .as_object()
            .and_then(|profile| profile.get("Default Bookmark"))
            .and_then(Value::as_bool)
            == Some(true)
    });
    let ordered: Vec<&Value> = match default_index {
        Some(index) => std::iter::once(&profiles[index])
            .chain(
                profiles
                    .iter()
                    .enumerate()
                    .filter_map(|(position, profile)| (position != index).then_some(profile)),
            )
            .collect(),
        None => profiles.iter().collect(),
    };
    for profile in ordered {
        let Some(profile) = profile.as_object() else {
            continue;
        };
        let (font_family, font_size) = read_macos_font_descriptor(profile.get("Normal Font"));
        let theme = read_iterm2_theme(profile);
        let detected = TerminalDetectedProfile {
            font_family,
            font_size,
            theme,
        };
        if has_detected_profile(&detected) {
            return Some(detected);
        }
    }
    None
}

pub fn detect_macos_terminal_profile_from_raw(plist: &Value) -> Option<TerminalDetectedProfile> {
    let plist = plist.as_object()?;
    let settings_names = [
        read_nested_string(plist.get("Startup Window Settings"), &[]),
        read_nested_string(plist.get("Default Window Settings"), &[]),
    ];
    for settings_name in settings_names.into_iter().flatten() {
        let Some(settings) = plist.get(&settings_name).and_then(Value::as_object) else {
            continue;
        };
        let font_family = read_nested_string(settings.get("FontName"), &[])
            .or_else(|| normalize_macos_font_name(settings.get("Font")))
            .or_else(|| read_macos_archived_font_name(settings.get("Font")));
        let (_, descriptor_font_size) = read_macos_font_descriptor(settings.get("Font"));
        let detected = TerminalDetectedProfile {
            font_family,
            font_size: normalize_font_size(settings.get("FontSize")).or(descriptor_font_size),
            theme: read_macos_terminal_theme(settings),
        };
        if has_detected_profile(&detected) {
            return Some(detected);
        }
    }
    None
}

pub fn terminal_detected_profile_to_json(profile: &TerminalDetectedProfile) -> Value {
    let mut entries = Vec::new();
    if let Some(font_family) = &profile.font_family {
        entries.push(("fontFamily".to_string(), json!(font_family)));
    }
    if let Some(font_size) = profile.font_size {
        entries.push(("fontSize".to_string(), number(font_size)));
    }
    if let Some(theme) = &profile.theme {
        entries.push((
            "theme".to_string(),
            Value::Object(
                theme
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            ),
        ));
    }
    Value::Object(entries.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_font_descriptor_and_color() {
        let (font_family, font_size) = read_macos_font_descriptor(Some(&json!("MesloLGS-NF 14")));
        assert_eq!(font_family.as_deref(), Some("MesloLGS NF"));
        assert_eq!(font_size, Some(14.0));
        assert_eq!(
            normalize_macos_color(Some(
                &json!({"Red Component":255,"Green Component":128,"Blue Component":0,"Alpha Component":0.5})
            )),
            Some("rgba(255, 128, 0, 0.5)".to_string())
        );
    }
}
