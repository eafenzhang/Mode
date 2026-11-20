//! IM bots 域入站解析适配层（W5 B2）。
//!
//! 规范：`docs/specs/bots-domain-migration.md` §2。transport crate 保持协议纯；
//! 本层负责「平台载荷 → provider 中立消息」的投影与发送目标判定。
//! B2a：钉钉；B2b：飞书/企微/微信入站解析。发送路径（HTTP + token）在 B2c。

#![forbid(unsafe_code)]

pub mod dingtalk;
pub mod feishu;
mod util;
pub mod wecom;
pub mod weixin;
