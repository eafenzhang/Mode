//! 更新链（路线图 M1 任务 6）。本阶段只落最小面，规格见
//! `docs/specs/tauri-platform-commands.md`。
//!
//! 目标：与旧 Electron 线**共用同一个 GitHub Releases feed**、版本号延续
//! （`electron-updater` 的 `latest.yml` 与 Tauri updater 的 `latest.json` 是两套格式，
//! 共用 feed 时两者并存，本阶段只做格式对接的占位）。
