//! 插件宿主资产：manifest 校验 + 按需下载 + 缓存（M3 生态域 1 · Task 1）。
//!
//! 规格：`docs/plans/2026-10-18-m3-plugin-host.md`（DR-PH-1…5）与
//! `docs/specs/plugin-host.md`。
//!
//! 本 crate 只做**资产**这一层：不发协议、不 spawn sidecar（Task 2 起）。
//! 资产是单文件 bundle（Node 运行时随 Host 发布，见 DR-M3-2），因此缓存层不做解包。

pub mod cache;
pub mod manifest;
pub mod protocol;
pub mod supervisor;

pub use cache::{AssetCache, CacheError};
pub use manifest::{ManifestError, PluginHostManifest};
