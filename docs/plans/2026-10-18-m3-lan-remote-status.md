# M3 生态域 2：LAN / 远程 / 远程桌面——以 Host sidecar 复用为主的验收

状态：2026-10-18 首版。**本文件是验收口径与现状，不是重写计划。**

## 结论先说

- M3 的 LAN 发现/配对/attach、SSH/WSL/Docker 远程、远程桌面**不需要在 Rust 侧重写**：
  Tauri 壳的 Host sidecar 复用了 `packages/services`（M3 DR-M3-1/DR-M3-2），这些能力已在
  渲染层与 Host 服务里；路线图 M3 列的 `crates/lan` / `crates/remote` / `crates/server`
  属于 **M4 的全 Rust 重写**范围，提前实现会与 Host 服务形成两条写入路径。
- 本轮实跑相关测试：**38 条全绿**（7 条 LAN/配对 + 31 条远程桌面/远端会话），说明服务层
  行为没有因 Tauri 迁移而回退。
- 仍缺的是**两台机器/真实 UI 的人工验收**，不是代码缺口。

## 2026-10-19 更正：Tauri 产品壳仍有代码缺口

上面「不是代码缺口」只适用于 Electron 服务层。Tauri 产品壳的 `window.mode` 仍把 LAN/RD
方法列为拒绝/空操作，`connectRemote` 直接拒绝；`host-standalone` 也没有启动 LAN/RD/remote
registry。真机证据：`.plan/browser-import-e2e-4.log:4395`（`not_implemented_until_m3:
connectRemote`）。该缺口已独立立项，见
`docs/plans/2026-10-19-lan-remote-sidecar-migration.md`；本文件的 M3 人工验收项在迁移完成前
不能作为「Tauri 版已可用」的依据。

## 本轮自动化证据

```bash
pnpm exec tsx --test \
  packages/desktop/tests/lan-access-disabled-state.test.mts \
  packages/desktop/tests/lan-attach-initialize.test.mjs \
  packages/desktop/tests/lan-materialize.test.mts \
  packages/desktop/tests/pair-label.test.mjs \
  packages/desktop/tests/remote-desktop-capability.test.mts \
  packages/desktop/tests/remote-target-match.test.mjs
# 7 pass / 0 fail

pnpm exec tsx --test \
  packages/desktop/tests/remote-desktop.test.mts \
  packages/desktop/tests/remote-desktop-input-path.test.mts \
  packages/desktop/tests/remote-desktop-monitors.test.mts \
  packages/desktop/tests/remote-desktop-media-relay.test.mts \
  packages/desktop/tests/remote-workspace-session-services.test.mts \
  packages/desktop/tests/lan-prompt-attachments.test.mts
# 31 pass / 0 fail
```

覆盖到的关键口径：

- LAN attach 的 Initialize 帧不因监听晚挂丢失、首条 RPC 在超时内完成；
- `target` 判等只认 host+port（token 不参与），ssh/wsl/docker 口径不变；
- 关闭开关仍回配对记录（含远程桌面授权），回帖 schema 可解析；
- `pairLanPeer` 的 label 兜底；
- 远程桌面能力位（平台 + kill-switch + 被控开关）、被控会话状态机、输入换算、
  main↔host 消息契约、媒体面路由、编码能力缓存；
- 剪贴板上下行、断连窗口去重、会话收口与开关关闭的作废语义；
- LAN 新对端的 CUA 权限观察归属（无 capabilities 时回落本机）。

## M3 仍需人工执行的验收（全部**必须由人**）

1. **两台机器**：A 发现 B → 输入 6 位配对码 → attach 到 B 的 workspace → 发一条 prompt →
   B 上执行 → A 看到流式结果。
2. **配对设备管理**：设置页「已配对设备」列表、工作区目录、删除后重连的行为
   （功能已在 v0.0.4 落地，缺 Tauri 版 UI 复核）。
3. **远程桌面**：主控/被控双向授权、剪贴板、多屏、断连重连；至少覆盖一台 Windows 被控端。
4. **旧对端降级**：`server-info` 缺 `computerUse` / capabilities 时 UI 维持「暂不支持」。

这些项需要第二台设备、真实的 LAN 服务和交互式授权，自动化不可替代；脚本会跳过。

## 与路线图的对应

| 路线图 M3 项 | 处置 |
| --- | --- |
| LAN 远连（UDP 发现 + 配对码 + 桌面即服务端） | 由 Host sidecar/services 复用；本轮测试全绿，待两台机器人工验收 |
| SSH/WSL/Docker 远程 + workspaceIdentity | 同上；`workspaceIdentity` 口径已由现有测试覆盖 |
| Web 端（axum）/ 手机远控 | 仍属新栈缺失：`packages/server` 已随 Host 可用，Rust `crates/server` 留 M4；手机链路待独立子计划 |
| TUI（ratatui） | 旧 `apps/mode-cli` TUI 仍可用；Rust TUI 留 M4 |

## 下一域

M3 剩余生态域：插件市场迁移收口、IM bot、Web/手机远控、TUI。建议下一个子计划做
**IM bot 逐通道**（现有 `packages/services` 已有实现，先盘点再决定是否需要在 Rust 侧补）。
