//! 保序 JSON 值。
//!
//! serde_json 的 `Map` 默认按 BTreeMap 排序，无法表达 TS 对象/Map 的插入顺序。
//! 配置解析只接受 JSON 输入，并且属性顺序是 payload 的一部分，所以这里用
//! `IndexMap` 承载对象；`Serialize` 手写为普通 JSON（不是枚举标签）。

use std::fmt;

use indexmap::IndexMap;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Number;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JsonValue>),
    Object(IndexMap<String, JsonValue>),
}

impl JsonValue {
    /// 按参数顺序构造对象。
    pub fn object_from(entries: impl IntoIterator<Item = (String, JsonValue)>) -> Self {
        let mut map = IndexMap::new();
        for (key, value) in entries {
            map.insert(key, value);
        }
        JsonValue::Object(map)
    }

    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).expect("JSON 值序列化不会失败")
    }

    pub fn kind(&self) -> &'static str {
        match self {
            JsonValue::Null => "null",
            JsonValue::Bool(_) => "boolean",
            JsonValue::Number(_) => "number",
            JsonValue::String(_) => "string",
            JsonValue::Array(_) => "array",
            JsonValue::Object(_) => "object",
        }
    }

    pub fn as_object(&self) -> Option<&IndexMap<String, JsonValue>> {
        match self {
            JsonValue::Object(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            JsonValue::Number(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsonValue::Null)
    }
}

impl Serialize for JsonValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            JsonValue::Null => serializer.serialize_unit(),
            JsonValue::Bool(value) => serializer.serialize_bool(*value),
            JsonValue::Number(value) => value.serialize(serializer),
            JsonValue::String(value) => serializer.serialize_str(value),
            JsonValue::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
            JsonValue::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct JsonValueVisitor;

        impl<'de> Visitor<'de> for JsonValueVisitor {
            type Value = JsonValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("任意 JSON 值")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(JsonValue::Null)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(JsonValue::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                JsonValue::deserialize(deserializer)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(JsonValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(JsonValue::Number(Number::from(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(JsonValue::Number(Number::from(value)))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Number::from_f64(value)
                    .map(JsonValue::Number)
                    .ok_or_else(|| E::custom("JSON 不支持 NaN/Infinity"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(JsonValue::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(JsonValue::String(value))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(JsonValue::Array(values))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = IndexMap::new();
                while let Some((key, value)) = map.next_entry::<String, JsonValue>()? {
                    values.insert(key, value);
                }
                Ok(JsonValue::Object(values))
            }
        }

        deserializer.deserialize_any(JsonValueVisitor)
    }
}

/// 构造对象；参数顺序即序列化顺序。
pub fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut map = IndexMap::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    JsonValue::Object(map)
}

pub fn array(values: Vec<JsonValue>) -> JsonValue {
    JsonValue::Array(values)
}

pub fn string(value: impl Into<String>) -> JsonValue {
    JsonValue::String(value.into())
}

pub fn boolean(value: bool) -> JsonValue {
    JsonValue::Bool(value)
}

pub fn int(value: i64) -> JsonValue {
    JsonValue::Number(Number::from(value))
}

/// zod `z.number().int()` 同时要求安全整数。返回 `None` 表示不是安全整数。
pub fn safe_integer(value: &Number) -> Option<i64> {
    const MAX_SAFE: i64 = 9_007_199_254_740_991;
    if let Some(value) = value.as_i64() {
        return (value.abs() <= MAX_SAFE).then_some(value);
    }
    if let Some(value) = value.as_u64() {
        return (value <= MAX_SAFE as u64).then_some(value as i64);
    }
    let value = value.as_f64()?;
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE as f64 {
        Some(value as i64)
    } else {
        None
    }
}
