# Tutto：用户数据根 `.mode → .tutto` 迁移清单（2026-10-19）

> 属于 P3 spec S3；涉及用户数据，必须单独窗口执行并验证迁移失败回退。
> 当前契约：`packages/shared/src/data-root.ts`。

## 1. 常量切换

| 常量       | 现在              | 改成               | 兼容                                        |
| ---------- | ----------------- | ------------------ | ------------------------------------------- |
| 数据根目录 | `.mode`           | `.tutto`           | 旧根 `.mode` / `.zcodium` / `.zcode` 仅迁移 |
| 归属文件   | `.mode-root.json` | `.tutto-root.json` | 读旧文件判定历史根                          |
| 产品 id    | `dev.mode.app`    | `dev.tutto.app`    | 接受 `dev.mode.app` / `dev.zcodium.app`     |
| 迁移动作   | copy 旧根 → 新根  | 不变               | 失败保留旧根，不回写、不删除                |

## 2. 需要同步的落点

- `packages/shared/src/data-root.ts`（常量、legacy 列表、`isAcceptedDataRootProduct`）；
- `packages/services` / `packages/desktop` / `apps/mode-cli` 中写死 `.mode` 的路径与提示文案；
- Electron userData 已改为 `Tutto`（S1），需确认与数据根迁移互不依赖；
- oracle 脚本：`isolation.test.mts`、`probe-plugin-roots.mts`、`probe-plugin-sidecar.mts`、
  `probe-bash-background.mts`、`probe-write-support.mts` 等把 `.mode` 改为 `.tutto`；
- 文档：`docs/specs/data-root*`、README 数据目录说明、迁移提示 i18n。

## 3. 迁移语义（不得更改）

1. 新根不存在、旧根存在 → copy 旧根到新根，写新归属文件；
2. 新旧都存在 → 用新根，不合并、不覆盖；
3. copy 失败 → 继续用旧根（`fallback`），UI 提示真实路径；
4. 旧根永不删除；用户手动清理。

## 4. 测试矩阵

- 全新环境：创建 `.tutto` + `.tutto-root.json`；
- 仅 `.mode`：迁移后设置/机器人/凭证/插件仍在；
- 仅 `.zcodium`/`.zcode`：多代迁移仍可达；
- `.tutto` 已存在且 `.mode` 也存在：优先新根，旧根不动；
- 归属产品 id：`dev.tutto.app`、`dev.mode.app`、`dev.zcodium.app` 均接受，未知名拒绝；
- copy 失败注入：回退旧根并可继续启动。

## 5. 验证

`pnpm typecheck`、`pnpm --filter @mode/shared test`、`pnpm --filter @mode/services test`、
desktop CLI 的 data-root 相关测试、`pnpm architecture:check --changed`、语言政策；
本地提交，不推送。

## 6. 风险

- 迁移方向写反会造成「新根空、旧数据看不到」；
- 归属文件产品 id 校验漏旧值会让历史根被判 unowned；
- oracle 脚本漏改会把测试数据写进真实 `.mode`。

## 7. 执行前盘点（2026-10-19，只读核对源码，未改动）

数据根有两类同名 `.mode` 目录，**不得混改**：

1. **用户级数据根** `{base}/.mode`（本次 S3 范围）：凭证、日志、db、telemetry、
   setting.json、全局 workflows、hooks 用户配置、MCP 用户目录、CLI 插件缓存等；
2. **项目级命名空间** `<workspace>/.mode/`（保留到 S5）：commands、skills、agents、
   workflows、workflow-drafts/runs、config.json、AGENTS.md、agent-memory。

归属契约已集中：`packages/shared/src/data-root.ts`、`packages/services/src/paths.ts`、
`packages/services/src/data-root/{ownership,migration}.ts`、`packages/desktop/src/host-standalone/dataRoot.ts`
以及 `crates/sidecar-host/src/lib.rs` 的文档口径，共 6 个文件引用
`MODE_DATA_ROOT_DIR_NAME`。

用户级根仍有约 30 处字面量 `join(homedir()/dataBaseDir, ".mode", …)`，分布：

- `apps/mode-cli/packages/adapters`：auth、device、exec、logging、model/runner-debug、
  storage/session-store、workspace-hook-trust-store、workflow；
- `apps/mode-cli/packages/cli`：clipboard-image、provider-runtime-env、sea-runtime-tools；
- `apps/mode-cli/packages/debug/server`、`packages/contracts/src/config`；
- `packages/desktop/src/main`：setting.json 引导、Chromium 硬件加速引导、exportLogs、
  mcpUserDirectory、desktopRuntimeEnv 的 MODE_HOME 回退；
- `packages/services`：commands/device/hooks/mcp-sync 等用户目录段。

消费方必须同批切换：CLI 与 Desktop 共用 `{base}/.mode`，只改一侧会让迁移后的新根与
CLI 新写入分叉。测试侧 `packages/services/test/dataRoot*.test.ts`（34 处）、
`apps/desktop-tauri/test/{portRoundTrip,realServices,webviewRoundTrip}.test.mts`、
`packages/desktop/tests/agent-runtime-oracle/*` 需要同步改为 `.tutto`，并保留至少一条
「旧 `.mode` 可迁移」的回归用例。

建议执行顺序（同一窗口内完成）：共享常量与 legacy 链 → shared/services 测试 →
desktop 与 CLI 用户级路径 → oracle/desktop-tauri 测试 → spec 与 README 文案 → 全量验证。

## 8. 第一阶段已落地（2026-10-19）

用户级路径字面量已收敛到 `@mode/shared` 的 `MODE_DATA_ROOT_DIR_NAME`（常量值本阶段
仍为 `.mode`，行为零变化）：mode-cli 的 adapters/cli/bootstrap/debug、desktop main、
services（commands/device/hooks/mcp-sync/mode-agent/plugin-sync/runtime-tools/setting/
settings-sync/skill-sync/skills/storage/subagents）、mode-server-cli 与 plugin-host。
项目级 `<workspace>/.mode/`（commands/skills/agents/workflows/plans/config 等）保持字面量，
留给 S5 目录改名。

下一阶段切换常量值时仍需同步：`packages/shared/src/data-root.ts`（目录名、归属文件、
产品 id 与 legacy 链）、`packages/desktop/src/host-standalone/dataRoot.ts` 的重复常量、
`crates/sidecar-host/src/lib.rs`、Rust/TS 测试与 oracle 中的 `.mode` 夹具，以及
`docs/specs/mode-data-root.md` 与 README。

## 9. 当前硬阻塞（2026-10-19 实测）

切换尝试在 `packages/mode-cua/**` 撞到并行会话所有权边界：`broker-server.js` 直接读
`~/.mode/cli/config.json`（用户层）与 `<workspace>/.mode/config.json`（项目层），没有走
`@mode/shared` 常量。若只切其他包，设置页写入 `~/.tutto/cli/config.json` 后 CUA 仍读旧根，
「电脑控制」开关会静默失效（`cua-enablement` 用例当场变红）。

因此常量切换必须与 CUA 包同批执行，而该目录当前归并行会话。已回退本轮未提交的常量
切换，只保留无行为变化的前置收敛；CUA 一期收口后按第 8 节清单一次性切换。

顺带修复：`discoverLegacyDataRootCandidates` 原先只探测 legacy 列表的第一项，现按
`LEGACY_MODE_DATA_ROOT_DIR_NAMES` 由新到旧取第一个存在的根（多代迁移一致）。已补回归：
只有末代 `.zcode` 时仍能发现；多代同时存在时取列表中最新的根。
