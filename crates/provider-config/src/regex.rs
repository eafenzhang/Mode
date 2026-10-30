//! `patternSchema` 的正则可接受性检查。
//!
//! TS 的判定是 `new RegExp("^(?:" + pattern + ")$")`。这里只覆盖配置里常见的
//! 语法（分组、字符类、量词、命名组），不实现完整 V8 文法；差异见 spec §5。

use std::collections::HashSet;

pub fn is_valid_js_regex_pattern(pattern: &str) -> bool {
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut group_depth = 0usize;
    let mut in_class = false;
    let mut can_quantify = false;
    let mut named_groups = HashSet::new();

    while index < chars.len() {
        let character = chars[index];

        if in_class {
            match character {
                '\\' => {
                    if index + 1 >= chars.len() {
                        return false;
                    }
                    index += 2;
                }
                ']' => {
                    in_class = false;
                    can_quantify = true;
                    index += 1;
                }
                _ => {
                    index += 1;
                }
            }
            continue;
        }

        match character {
            '\\' => {
                if index + 1 >= chars.len() {
                    return false;
                }
                can_quantify = true;
                index += 2;
            }
            '[' => {
                in_class = true;
                can_quantify = false;
                index += 1;
            }
            '(' => {
                if chars.get(index + 1) == Some(&'?') {
                    match chars.get(index + 2) {
                        Some(':') | Some('=') | Some('!') => {
                            index += 3;
                        }
                        Some('<') => match chars.get(index + 3) {
                            Some('=') | Some('!') => {
                                index += 4;
                            }
                            _ => {
                                let mut cursor = index + 3;
                                let mut name = String::new();
                                while cursor < chars.len() && chars[cursor] != '>' {
                                    name.push(chars[cursor]);
                                    cursor += 1;
                                }
                                if cursor >= chars.len()
                                    || name.is_empty()
                                    || !is_valid_group_name(&name)
                                    || !named_groups.insert(name)
                                {
                                    return false;
                                }
                                index = cursor + 1;
                            }
                        },
                        _ => return false,
                    }
                } else {
                    index += 1;
                }
                group_depth += 1;
                can_quantify = false;
            }
            ')' => {
                if group_depth == 0 {
                    return false;
                }
                group_depth -= 1;
                can_quantify = true;
                index += 1;
            }
            '|' => {
                can_quantify = false;
                index += 1;
            }
            '*' | '+' | '?' => {
                if !can_quantify {
                    return false;
                }
                can_quantify = false;
                index += 1;
            }
            '{' => match parse_quantifier(&chars, index) {
                Some((end, minimum, maximum)) => {
                    if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
                        if minimum > maximum {
                            return false;
                        }
                    }
                    can_quantify = false;
                    index = end;
                }
                None => {
                    can_quantify = true;
                    index += 1;
                }
            },
            '^' | '$' => {
                // `^{1}` 合法，但 `^*` 非法：量词块不检查 can_quantify。
                can_quantify = false;
                index += 1;
            }
            _ => {
                can_quantify = true;
                index += 1;
            }
        }
    }

    !in_class && group_depth == 0
}

fn is_valid_group_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// 解析 `{n}` / `{n,}` / `{n,m}`；返回 (结束下标, 最小, 最大)。
/// 不是合法量词时返回 `None`（非 unicode 模式下 `{` 是字面量）。
fn parse_quantifier(chars: &[char], start: usize) -> Option<(usize, Option<u64>, Option<u64>)> {
    let mut cursor = start + 1;
    let minimum = read_digits(chars, &mut cursor)?;
    if chars.get(cursor) == Some(&'}') {
        return Some((cursor + 1, Some(minimum), Some(minimum)));
    }
    if chars.get(cursor) != Some(&',') {
        return None;
    }
    cursor += 1;
    if chars.get(cursor) == Some(&'}') {
        return Some((cursor + 1, Some(minimum), None));
    }
    let maximum = read_digits(chars, &mut cursor)?;
    if chars.get(cursor) != Some(&'}') {
        return None;
    }
    Some((cursor + 1, Some(minimum), Some(maximum)))
}

fn read_digits(chars: &[char], cursor: &mut usize) -> Option<u64> {
    let start = *cursor;
    let mut value = 0u64;
    while let Some(character) = chars.get(*cursor) {
        let Some(digit) = character.to_digit(10) else {
            break;
        };
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
        *cursor += 1;
    }
    (*cursor > start).then_some(value)
}
