//! IM bots 域入站解析适配层（W5 B2）。
//!
//! 规范：`docs/specs/bots-domain-migration.md` §2。transport crate 保持协议纯；
//! 本层负责「平台载荷 → provider 中立消息」的投影与发送目标判定。
//! B2a（本波）：钉钉；其余三通道按同一口径逐波移植。

#![forbid(unsafe_code)]

pub mod dingtalk;
