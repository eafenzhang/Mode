//! Task 4 `raw_input` 解析器测试（纯函数面：**不注入真实输入**）。
//!
//! 真实注入（SendInput）不进自动化：本文件只钉解析边界与空序列的派发计数，
//! 实机注入在 T7 两机验收按 `docs/specs/lan-remote-desktop.md` 场景执行。
//!
//! 口径（brief A 节）：逐条严格校验——未知键 / 未知 k / 缺字段 / 越界一律
//! `invalid_request`；`events` ≤ 512 条、`text` ≤ 4096 字符、wheel `amount` ≤ 20。
use mode_cua_ax::input::{
  dispatch_raw_input, parse_raw_input, MouseButton, RawInputEvent, WheelDirection,
  MAX_RAW_INPUT_EVENTS, MAX_RAW_TEXT_CHARS,
};
use serde_json::{json, Value};

/// 解析失败并返回错误码（成功即 panic——用例全部断言失败路径）。
fn code_of(events: Value) -> String {
  parse_raw_input(&events)
    .map(|_| panic!("预期 invalid_request，实际解析通过"))
    .unwrap_or_else(|e| e.code)
    .to_string()
}

// ————————————————————————— 正向：五类事件的规范化形态 —————————————————————————

#[test]
fn raw_events_parse_to_normalized_form() {
  let parsed = parse_raw_input(&json!([
    { "k": "move", "x": 1, "y": 2 },
    { "k": "button", "button": "right", "down": true, "x": 3, "y": 4 },
    { "k": "wheel", "direction": "left", "amount": 2.5, "x": 5, "y": 6 },
    { "k": "key", "vk": 65, "down": false },
    { "k": "text", "text": "中文abc" },
  ]))
  .expect("合法事件序列应解析通过");
  assert_eq!(parsed.len(), 5);
  assert_eq!(parsed[0], RawInputEvent::Move { x: 1, y: 2 });
  assert_eq!(
    parsed[1],
    RawInputEvent::Button { button: MouseButton::Right, down: true, x: 3, y: 4 }
  );
  assert_eq!(
    parsed[2],
    RawInputEvent::Wheel { direction: WheelDirection::Left, amount: 2.5, x: 5, y: 6 }
  );
  assert_eq!(parsed[3], RawInputEvent::Key { vk: 65, down: false });
  assert_eq!(parsed[4], RawInputEvent::Text { text: "中文abc".to_string() });
}

/// 边界值必须收（0/65535 是 VIRTUALDESK 归一化域的闭区间端点；vk 1..=255 同理）。
#[test]
fn boundary_values_are_accepted() {
  let parsed = parse_raw_input(&json!([
    { "k": "move", "x": 0, "y": 65535 },
    { "k": "button", "button": "middle", "down": false, "x": 65535, "y": 0 },
    { "k": "wheel", "direction": "down", "amount": 20, "x": 0, "y": 0 },
    { "k": "wheel", "direction": "up", "amount": 0, "x": 1, "y": 1 },
    { "k": "key", "vk": 1, "down": true },
    { "k": "key", "vk": 255, "down": true },
  ]))
  .expect("边界值应解析通过");
  assert_eq!(parsed.len(), 6);
  assert_eq!(parsed[0], RawInputEvent::Move { x: 0, y: 65535 });
  assert_eq!(parsed[1], RawInputEvent::Button { button: MouseButton::Middle, down: false, x: 65535, y: 0 });
  assert_eq!(parsed[2], RawInputEvent::Wheel { direction: WheelDirection::Down, amount: 20.0, x: 0, y: 0 });
  assert_eq!(parsed[3], RawInputEvent::Wheel { direction: WheelDirection::Up, amount: 0.0, x: 1, y: 1 });
}

// ————————————————————————— 反向：未知 k / 缺字段 / 未知键 —————————————————————————

#[test]
fn unknown_kind_and_shape_errors_reject() {
  // 未知 k。
  assert_eq!(code_of(json!([{ "k": "jump" }])), "invalid_request");
  // k 缺失 / 非字符串。
  assert_eq!(code_of(json!([{ "x": 1, "y": 2 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": 7 }])), "invalid_request");
  // 事件项非对象。
  assert_eq!(code_of(json!([3])), "invalid_request");
  assert_eq!(code_of(json!(["move"])), "invalid_request");
  // 顶层不是数组（`{ events }` 的 events 字段由 napi 层取出后直接进本函数）。
  assert_eq!(code_of(json!("nope")), "invalid_request");
  assert_eq!(code_of(Value::Null), "invalid_request");
  assert_eq!(code_of(json!({ "k": "move", "x": 1, "y": 2 })), "invalid_request");
}

#[test]
fn missing_fields_reject() {
  // move 缺 y / 缺 x。
  assert_eq!(code_of(json!([{ "k": "move", "x": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "move", "y": 1 }])), "invalid_request");
  // button 缺 button/down/x/y 各一例。
  assert_eq!(code_of(json!([{ "k": "button", "down": true, "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(
    code_of(json!([{ "k": "button", "button": "left", "x": 1, "y": 1 }])),
    "invalid_request"
  );
  assert_eq!(
    code_of(json!([{ "k": "button", "button": "left", "down": true, "y": 1 }])),
    "invalid_request"
  );
  // wheel 缺 direction / amount / x / y。
  assert_eq!(code_of(json!([{ "k": "wheel", "amount": 1, "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "wheel", "direction": "up", "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(
    code_of(json!([{ "k": "wheel", "direction": "up", "amount": 1, "y": 1 }])),
    "invalid_request"
  );
  // key 缺 vk / down。
  assert_eq!(code_of(json!([{ "k": "key", "down": true }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "key", "vk": 65 }])), "invalid_request");
  // text 缺 text。
  assert_eq!(code_of(json!([{ "k": "text" }])), "invalid_request");
}

#[test]
fn unknown_keys_reject() {
  // 每类事件各带一个多余键：严格拒绝（防上游悄悄改名后语义静默漂移）。
  assert_eq!(code_of(json!([{ "k": "move", "x": 1, "y": 2, "z": 3 }])), "invalid_request");
  assert_eq!(
    code_of(json!([{ "k": "button", "button": "left", "down": true, "x": 1, "y": 2, "n": 1 }])),
    "invalid_request"
  );
  assert_eq!(
    code_of(json!([{ "k": "wheel", "direction": "up", "amount": 1, "x": 1, "y": 2, "d": 1 }])),
    "invalid_request"
  );
  assert_eq!(code_of(json!([{ "k": "key", "vk": 65, "down": true, "x": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "text", "text": "a", "x": 1 }])), "invalid_request");
}

#[test]
fn wrong_types_reject() {
  // down 必须布尔；vk/x/y 必须整数（1.5 会被 napi/JSON 侧静默截断成另一键位，必须拒）。
  assert_eq!(code_of(json!([{ "k": "key", "vk": 65, "down": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "button", "button": "left", "down": "true", "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "move", "x": 1.5, "y": 2 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "key", "vk": 65.5, "down": true }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "move", "x": "1", "y": 2 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "text", "text": 3 }])), "invalid_request");
  // button/direction 枚举外的值。
  assert_eq!(code_of(json!([{ "k": "button", "button": "back", "down": true, "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "wheel", "direction": "diag", "amount": 1, "x": 1, "y": 1 }])), "invalid_request");
}

#[test]
fn out_of_range_reject() {
  // VIRTUALDESK 归一化域 0..=65535。
  assert_eq!(code_of(json!([{ "k": "move", "x": 65536, "y": 0 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "move", "x": 0, "y": 65536 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "move", "x": -1, "y": 0 }])), "invalid_request");
  assert_eq!(
    code_of(json!([{ "k": "button", "button": "left", "down": true, "x": 70000, "y": 0 }])),
    "invalid_request"
  );
  // vk 1..=255（0 是 Unicode 通道的 wVk，不属于 VK 面）。
  assert_eq!(code_of(json!([{ "k": "key", "vk": 0, "down": true }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "key", "vk": 256, "down": true }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "key", "vk": -5, "down": true }])), "invalid_request");
  // wheel amount 0..=20（防暴走：一次 20 tick = 2400 delta 已是上限）。
  assert_eq!(code_of(json!([{ "k": "wheel", "direction": "up", "amount": 20.5, "x": 1, "y": 1 }])), "invalid_request");
  assert_eq!(code_of(json!([{ "k": "wheel", "direction": "up", "amount": -1, "x": 1, "y": 1 }])), "invalid_request");
}

#[test]
fn event_count_cap_enforced() {
  let at_cap: Vec<Value> = (0..MAX_RAW_INPUT_EVENTS).map(|i| json!({ "k": "move", "x": i % 65536, "y": 0 })).collect();
  assert_eq!(
    parse_raw_input(&json!(at_cap)).expect("512 条应通过").len(),
    MAX_RAW_INPUT_EVENTS
  );
  let over_cap: Vec<Value> = (0..MAX_RAW_INPUT_EVENTS + 1)
    .map(|i| json!({ "k": "move", "x": i % 65536, "y": 0 }))
    .collect();
  assert_eq!(code_of(json!(over_cap)), "invalid_request");
}

#[test]
fn text_length_cap_enforced() {
  let at_cap: String = "汉".repeat(MAX_RAW_TEXT_CHARS);
  let parsed = parse_raw_input(&json!([{ "k": "text", "text": at_cap }])).expect("4096 字符应通过");
  assert_eq!(parsed, vec![RawInputEvent::Text { text: at_cap }]);
  let over_cap: String = "汉".repeat(MAX_RAW_TEXT_CHARS + 1);
  assert_eq!(code_of(json!([{ "k": "text", "text": over_cap }])), "invalid_request");
}

// ————————————————————————— 空序列：零注入路径的计数契约 —————————————————————————

/// `{events: []}` 是远控空闲期的常态请求：解析出空序列、派发计数 0，
/// **不进 SendInput**（本用例不触碰真实输入设备，可安全进自动化）。
#[test]
fn empty_events_dispatch_zero() {
  let parsed = parse_raw_input(&json!([])).expect("空数组应解析通过");
  assert!(parsed.is_empty());
  assert_eq!(dispatch_raw_input(&parsed), 0);
}
