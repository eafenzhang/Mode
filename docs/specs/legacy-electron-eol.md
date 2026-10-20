# 旧 Electron 桌面线 EOL

状态：已批准（2026-10-20，用户决定「旧 Electron 线可以取消了」）。

## 决策

旧 Electron 桌面线（`packages/desktop/src/main`、`src/preload`、`src/scheduler`、Electron host
入口、electron-builder/electron-updater 打包与发布链）自 2026-10-20 起 EOL。仓库不再构建、
不再发布、不再保留其运行时代码；后续桌面端只维护 Tauri 线（`apps/desktop-tauri`）。

这是一次**不可回滚到旧线**的收口：回滚能力由 git 历史提供，不通过在 `release.yml` 保留
Electron 诊断构建提供。

## 必须保留（Tauri 仍依赖）

- `packages/desktop/src/renderer/**`：Tauri 产品 renderer 的唯一来源（含 `about.html`）。
- `packages/desktop/src/host-standalone/**`：Tauri Host sidecar 入口。
- `packages/desktop/src/host/` 中 host-standalone 可达的 LAN/远程桌面/远程工作区模块。
- `packages/desktop/scripts/{prepare-agent-node-bundle,stage-agent-bundle,build-metadata,desktop-product-identity}.mjs`。
- `packages/desktop/bundled-agents/**`、`bundled-tools/win32-x64/cua-helper/**`、
  `packages/desktop/build/icon.png`（fnOS 仍引用）。
- `packages/desktop/vite.config.ts`、`tsconfig.renderer.json`、`tsconfig.host-standalone.json`。
- 根脚本 `build:cua-helper`、`stage:cua-helper`、`build:plugin-host`、协议生成与 Tauri 构建链。
- fnOS 发布 job（不是 Electron 产物，仅复用 `build/icon.png`）。

## 必须删除

- Electron 运行时：`packages/desktop/src/main/**`、`src/preload/**`、`src/scheduler/**`。
- Electron host 入口与仅被它引用的 `src/host` 模块（含 `host/index.ts`、`electronPort.ts`、
  `storagePreparationProcesses.ts`、`tasksStorageWorker.ts` 等）。
- Electron-only renderer 页面：resource-manager、cua-permission-panel、data-root-decision。
- Electron 构建配置：`electron-builder.config.js`、`dev-app-update.yml`、`tsup.config.ts`、
  `tsconfig.main.json`、`tsconfig.preload.json`、`tsconfig.scheduler.json`、
  `tsconfig.host.json`。
- Electron 开发/打包脚本与根脚本（`dev:desktop*`、`bundle:desktop`、
  `prepare:desktop-runtime`、`build:desktop-agent:bytecode` 等）。
- `.github/workflows/release.yml` 的 `desktop` job 与 `electron-checksums-*` 产物。
- Electron 专属测试（`src/main` 引用、electron-builder 断言、Electron 更新/策略测试）。
- Electron 依赖（`electron`、`electron-builder`、`electron-updater`、`@electron/*` 等）。

## 保留但改写

- `remote-desktop.test.mts` 只保留 host 侧编码能力缓存夹具；main 侧状态机/消息镜像夹具删除。
- `upstream-overlap-metric.test.mts` 删除 `src/main/index.ts` 路径断言。
- 政策文档 `packages/desktop/specs/no-official-platform.md`、`no-telemetry.md` 保留，
  但不再用 Electron main 源码做断言；如需要门禁，改对 Tauri/Host 源码。

## 验收

1. `release.yml` 不再有 Electron 构建/发布 job；`fnos` 与三条 Tauri 腿保持不变。
2. `packages/desktop` 不再声明 `main`/Electron 依赖；`pnpm install` 不再执行 Electron ABI rebuild。
3. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、语言政策通过。
4. `pnpm --filter @mode/desktop-tauri test` 与保留的 `packages/desktop/tests/*.test.mts` 通过。
5. `pnpm --filter @mode/desktop-tauri run build:package-inputs` 能重新构建 product renderer
   与 Host sidecar。
6. 文档中不再把 Electron 线写成当前可用的桌面端或回滚路径；历史计划保留并标注已被本 spec
   取代。

## 已知后续项

- `packages/desktop/tsconfig.renderer.json` 目前不在根 `typecheck` 里：一旦接入，会暴露
  `window.mode` 声明与 `IPlatformService` 之间的既有缺口（`createTempTextAttachment`、
  `renameLanPairedPeer`、`getWindowControlsOverlayMetrics` 等）。本 spec 不把它扩大成
  类型重构；待 Tauri 平台面收敛后单独补。
- `packages/desktop/tests/remote-desktop.test.mts` 只保留 host 侧输入换算与编码能力缓存；
  main 侧会话状态机/消息镜像覆盖随 Electron 一起删除。
- 旧 Electron 的 CI 诊断步骤（browser-import C# oracle 对拍）已搬进 `tauri` job 的
  Windows 腿，作为 `continue-on-error` 辅助验证；M0 P2 收口后应重新评估是否转正。

## 执行记录（2026-10-20，提交 `d71ed992`）

已删除：

- `packages/desktop/src/{main,preload,scheduler}`；
- Electron-only `src/host` 模块（33 个，含 `index.ts`、`electronPort.ts`、
  `storagePreparationProcesses.ts`、`tasksStorageWorker.ts`）；
- Electron-only renderer 页面（resource-manager、cua-permission-panel、data-root-decision）；
- `electron-builder.config.js`、`dev-app-update.yml`、`tsup.config.ts`、
  `tsconfig.{main,preload,scheduler,host}.json`、`tsconfig.json`；
- Electron 开发/打包脚本与根脚本、`.vscode` Electron 调试配置；
- Electron 专属测试与 main 侧远程桌面夹具；
- `release.yml` 的 `desktop` job 与 `electron-checksums-*` 产物；
- Electron 依赖（`electron`、`electron-builder`、`electron-updater`、`@electron/*`、
  `@fiahfy/icns`、`tsup`、`http-mitm-proxy`）；
- `modeAgentProcessManager` 的 Electron 字节码分支（`MODE_DESKTOP_AGENT_BYTECODE` /
  `mode.bytecode.cjs`），Tauri 线统一走 `mode.cjs`。

保留并验证：

- product renderer、`src/host-standalone`、Tauri 可达的 22 个 `src/host` 模块；
- `prepare:agent-bundle`、`build-metadata`、`desktop-product-identity`、agent/CUA 产物；
- `release.yml` 的 `tauri` 三平台 job、`fnos` job、`checksums` / `publish`；
- 根脚本 `dev:desktop` 改为 Tauri `cargo tauri dev`。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `pnpm typecheck` | 通过 |
| `pnpm lint` | 0 error / 82 warning |
| `pnpm architecture:check --changed` | 0 violations / 0 new |
| `node scripts/check-language-policy.mjs` | 通过（227 受管 / 65 vendored / 162 清单） |
| `pnpm exec tsx --test packages/desktop/tests/*.test.{mjs,mts}` | 160 passed / 3 skipped / 0 failed |
| `pnpm --filter @mode/services test`（去掉继承的 `MODE_DATA_BASE_DIR`） | 392 passed / 8 skipped / 0 failed |
| `pnpm --filter @mode/desktop-tauri test` | 39 passed / 7 skipped / 0 failed |
| `pnpm --filter @mode/desktop-tauri run build:package-inputs` | 通过（renderer 49.37 MiB + Host bundle 5.47 MiB） |
| `node scripts/check-language-policy.mjs`（2026-10-21 清单清理后） | 通过（193 受管 / 65 vendored / 128 清单 / 0 陈旧） |

2026-10-21 补充：`scripts/language-policy-allowlist.txt` 删除 34 条随旧线删除的陈旧条目
（162→128 条，陈旧 0）；`pnpm check:upstream-overlap` 复测 headline 9.20%
（37,241 / 404,926 行，分母因旧线删除从 462,066 降至 404,926）。

未执行/未覆盖：真实安装包启动、macOS/Linux 出包复验。
