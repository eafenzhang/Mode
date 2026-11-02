//! 终端字体/主题 profile 纯组合。
//!
//! 真源：`packages/services/src/terminal/terminalProfile.ts` 的
//! `composeTerminalFontProfile` 与字体栈 helper。系统探测结果由调用方注入。

use indexmap::IndexMap;
mod macos_profile;
mod system_profile;
use serde_json::{json, Number, Value};

fn json_number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

pub const FONT_FAMILY_FALLBACKS: [&str; 12] = [
    "ui-monospace",
    "SFMono-Regular",
    "SF Mono",
    "Menlo",
    "Monaco",
    "Consolas",
    "Cascadia Mono",
    "JetBrains Mono",
    "MesloLGS NF",
    "Hack Nerd Font",
    "Noto Sans Mono CJK SC",
    "monospace",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFontFamilySource {
    Custom,
    System,
    Fallback,
}

impl TerminalFontFamilySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::System => "system",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalDetectedProfile {
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub theme: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalFontProfile {
    pub font_family: String,
    pub font_size: Option<f64>,
    pub theme: Option<IndexMap<String, String>>,
    pub source: TerminalFontFamilySource,
}

pub fn normalize_font_family(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn dedupe_font_family_stack(primary: &str) -> String {
    let mut stack: Vec<String> = primary
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    for fallback in FONT_FAMILY_FALLBACKS {
        if !stack.iter().any(|item| item == fallback) {
            stack.push(fallback.to_string());
        }
    }
    stack.join(", ")
}

pub fn compose_terminal_font_profile(
    settings_font_family: Option<&str>,
    detected_profile: Option<&TerminalDetectedProfile>,
) -> TerminalFontProfile {
    if let Some(custom_font_family) = normalize_font_family(settings_font_family) {
        return TerminalFontProfile {
            font_family: dedupe_font_family_stack(&custom_font_family),
            font_size: detected_profile.and_then(|profile| profile.font_size),
            theme: detected_profile.and_then(|profile| profile.theme.clone()),
            source: TerminalFontFamilySource::Custom,
        };
    }
    if let Some(detected_profile) = detected_profile {
        return TerminalFontProfile {
            font_family: dedupe_font_family_stack(
                detected_profile
                    .font_family
                    .as_deref()
                    .unwrap_or(FONT_FAMILY_FALLBACKS[0]),
            ),
            font_size: detected_profile.font_size,
            theme: detected_profile.theme.clone(),
            source: TerminalFontFamilySource::System,
        };
    }
    TerminalFontProfile {
        font_family: FONT_FAMILY_FALLBACKS.join(", "),
        font_size: None,
        theme: None,
        source: TerminalFontFamilySource::Fallback,
    }
}

pub use macos_profile::{
    detect_iterm2_profile_from_raw, detect_macos_terminal_profile_from_raw,
    terminal_detected_profile_to_json,
};
pub use system_profile::{
    detect_kitty_font_family_from_raw, detect_vs_code_terminal_font_family_from_raw,
    detect_windows_terminal_font_family_from_raw, parse_jsonc, read_terminal_config_path,
};

impl TerminalDetectedProfile {
    pub fn is_empty(&self) -> bool {
        self.font_family.is_none() && self.font_size.is_none() && self.theme.is_none()
    }
}

impl TerminalFontProfile {
    pub fn to_json(&self) -> Value {
        let mut entries = vec![("fontFamily".to_string(), json!(self.font_family))];
        if let Some(font_size) = self.font_size {
            entries.push(("fontSize".to_string(), json_number(font_size)));
        }
        if let Some(theme) = &self.theme {
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
        entries.push(("source".to_string(), json!(self.source.as_str())));
        Value::Object(entries.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_stack_is_deduped_and_completed() {
        let profile = compose_terminal_font_profile(Some(" Menlo, ,JetBrains Mono,Menlo "), None);
        assert_eq!(profile.source, TerminalFontFamilySource::Custom);
        assert!(profile.font_family.starts_with("Menlo, JetBrains Mono, "));
        assert!(profile.font_family.ends_with(", monospace"));
    }

    #[test]
    fn detected_without_font_uses_primary_fallback() {
        let detected = TerminalDetectedProfile {
            font_family: None,
            font_size: Some(12.0),
            theme: None,
        };
        let profile = compose_terminal_font_profile(None, Some(&detected));
        assert_eq!(profile.source, TerminalFontFamilySource::System);
        assert_eq!(profile.font_size, Some(12.0));
        assert_eq!(
            profile.font_family,
            "ui-monospace, SFMono-Regular, SF Mono, Menlo, Monaco, Consolas, Cascadia Mono, JetBrains Mono, MesloLGS NF, Hack Nerd Font, Noto Sans Mono CJK SC, monospace"
        );
    }
}
