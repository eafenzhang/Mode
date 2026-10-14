//! Windows 浏览器数据导入 helper。
//!
//! 契约真源：`docs/specs/browser-import-helper-contract.md`。
//! 旧 C# 实现属上游衍生文件，按 `docs/specs/clean-room-policy.md` 禁读；
//! 本 crate 只从契约 spec、公开 API 文档与黑盒观察到的输入输出编写。

pub mod cli;
pub mod protocol;
