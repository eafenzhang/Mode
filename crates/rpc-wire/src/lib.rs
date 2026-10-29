//! `@mode/rpc` 二进制序列化（Layer 1）的 Rust 侧。
//!
//! 契约真源：`docs/specs/rpc-wire-format.md`；TS 实现：`packages/rpc/src/serialization.ts`。
//! 黄金字节由 `scripts/generate-rpc-wire-fixtures.ts` 产出，`tests/parity.rs` 逐字节对拍。
//!
//! 本 crate 只做编解码，不含传输、频道或流控——那些仍由 Node Host 与壳的现有链路负责，
//! 直到 M4b 把服务面搬进壳。

mod codec;
mod vql;

pub use codec::{deserialize, serialize, DecodeError};
pub use vql::{read_int_vql, write_int_vql};

/// 数据类型的 1 字节标签（与 TS `DataType` 枚举逐值对应）。
pub mod tag {
    pub const UNDEFINED: u8 = 0;
    pub const STRING: u8 = 1;
    /// JS `Uint8Array`
    pub const BUFFER: u8 = 2;
    /// `@mode/rpc` 的 `VSBuffer` 包装
    pub const VS_BUFFER: u8 = 3;
    pub const ARRAY: u8 = 4;
    /// JSON 回退：UTF-8 的 `JSON.stringify` 结果
    pub const OBJECT: u8 = 5;
    /// VQL 编码的 32 位整数
    pub const INT: u8 = 6;
}

/// 线上可表达的值。
///
/// `Buffer` 与 `Bytes` 是两个不同的 tag（`VSBuffer` vs `Uint8Array`），不能合并：
/// TS 侧 `instanceof` 的判定顺序决定了同一份字节会编成不同标签，合并会让两端字节不同。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Str(String),
    /// tag 2：JS `Uint8Array`
    Bytes(Vec<u8>),
    /// tag 3：`VSBuffer`
    Buffer(Vec<u8>),
    Int(i32),
    Array(Vec<Value>),
    /// tag 5：原样承载的 JSON 文本（不做解析，见 spec「Object 分支的 JSON 细节」）。
    Json(String),
}

impl Value {
    pub fn str(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }
}
