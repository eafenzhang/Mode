# 电脑控制插件分发（自研 mode-cua-plugin producer）

状态：已拍板（2026-10-10）。关联：[computer-use-enablement.md](./computer-use-enablement.md)（开关权威门）、[computer-use-windows-runtime.md](./computer-use-windows-runtime.md)（runtime 与 14 工具面规范）。

## 背景与问题

`computer-use@mode-plugins-official` 插件的种子资产（`.mode-plugin/plugin.json`、
`docs/computer-use.md`、`scripts/computer-use-client.mjs`、`skills/computer-use/SKILL.md`）
在仓库与桌面/远端打包中均不存在——`rootCandidates` 指向的 `mode-cua-plugin` producer 包从未纳入
本仓与三份 staging 清单。后果：seed 静默跳过 → 插件不在发现列表 → 设置页开关
`setPluginEnabled` 抛 `Plugin not found: computer-use@mode-plugins-official`（2026-10-10 桌面
实机日志三次），电脑控制整条链无法启用。

本 spec 定义自研 producer 包的契约与分发接线；内容全部自研（clean-room，取材清单见文末）。

## 包契约

位置：`apps/mode-cli/packages/mode-cua-plugin/`（pnpm workspace `apps/mode-cli/packages/*` 自动纳入）。

```text
mode-cua-plugin/
├── package.json                     # name: @mode/cua-plugin，version 对齐 0.6.3，无构建脚本
├── README.md
├── .mode-plugin/plugin.json         # {"name":"computer-use","version":"0.6.3"}，禁止 mcpServers
├── docs/computer-use.md             # 14 工具面规范（宿主经 MODE_CUA_PLUGIN_ROOT/docs 暴露）
├── scripts/computer-use-client.mjs  # 自研薄 SDK（kernel 可从插件根动态 import，browser 先例）
└── skills/computer-use/SKILL.md     # frontmatter name/description/when_to_use + 操作流程
```

- **rootCandidates 两态解析**：dev 由 bootstrap 模块目录经 `../../../mode-cua-plugin` 命中本包；
  打包态由 entrypoint（`resources/glm`）经 `packages/mode-cua-plugin` 命中 stage 产物
  （与 `verifyStagedKoffi` 既有默认路径一致）。
- **manifest 禁止 `mcpServers`**：宿主契约明示 mode-cua 为 skill/SDK-only，不生成独立 CUA MCP
  server；工具面经共享 `node_repl` 承载（`official-plugin-runtime.ts` rewrite 注释）。
- **版本对齐**：manifest/package 版本 = 官方定义 `version`（当前 0.6.3）= 市场条目版本，
  避免 comparePluginUpdate 永久报可更新。

## 分发接线（staging 清单）

| 清单                                                                              | 改动                                                                                                                                                                          |
| --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages/desktop/scripts/prepare-agent-node-bundle.mjs` `officialPluginPackages` | 新增 `{ packageName: "@mode/cua-plugin", relativePath: "apps/mode-cli/packages/mode-cua-plugin", stagedPath: "packages/mode-cua-plugin" }`（静态资产，无 runtimeBuildScript） |
| `scripts/prepare-prebuilds.mjs` `remoteOfficialPluginPackages`                    | 同上条目                                                                                                                                                                      |
| `scripts/prepare-prebuilds.mjs` `remoteOfficialPluginRequiredPaths`               | 新增 `packages/mode-cua-plugin/.mode-plugin/plugin.json`                                                                                                                      |
| `scripts/build-desktop-agent-cli.mjs`                                             | 不改：该清单只管 runtime 产物（本包无 runtime）                                                                                                                               |
| SEA                                                                               | 不改：`resolveSeaSeedSource` 由 definitions + 本地 seed 源派生，自动拾取                                                                                                      |

顶层白名单无需改：`docs`、`scripts`、`skills`、`package.json`、`.mode-plugin` 均已在
`includedTopLevelPaths` / `remoteOfficialPluginTopLevelPaths` 内。

新增 workspace 包会改 `pnpm-lock.yaml` importers——落地必须跑 `pnpm install`，否则 CI
frozen 安装失败。

## 内容契约（自研）

- `docs/computer-use.md`：14 个方法（`list_apps / list_windows / get_app_state / left_click /
scroll / left_click_drag / type / set_value / select_text / key / perform_action / paste /
request_access / stop_computer_control`）的签名与语义、observe→act 循环、收据与错误码
  （`CUA_NOT_READY`、`STALE_STATE`、`element_unavailable`、`foreground_required`、
  `permission_denied`、`not_settable` 等）、Windows 一期 / macOS fail-closed 平台矩阵。
  机械对照：docs 必须以反引号形式覆盖全部 14 名（单测执行）。
- `skills/computer-use/SKILL.md`：frontmatter `name: computer-use`；正文以
  **bridge global 零依赖路径**为首选配方（`globalThis[Symbol.for("mode.node-repl.computer-use-bridge")]`），
  动态 import client 为可选人体工学层；含子代理不可用、stop 会话闸门、锁屏与安全边界。
- `scripts/computer-use-client.mjs`：导出 `COMPUTER_METHOD_NAMES`（14，与 docs 机械对照）、
  只读四方法分类、`resolveCuaBridge` / `createComputerUseClient`（方法校验 + bridge 绑定）。
  纯 ESM、零依赖，kernel 动态 import 可用（browser-client 同机制，见 bundled-plugins 白名单注释）。

## 与宿主的门（不改宿主代码）

```text
插件 seed 存在 → 发现列表含 computer-use → 设置开关（权威门）写 enabledPlugins
  ├─ resolveBuiltInNodeReplMcpServers：启用才注册 node_repl，MODE_CUA_PLUGIN_ROOT=<root>
  ├─ resolvePluginRuntimeFeatures.computerUse=true → runtime-config 把 node_repl 加入
  │    cuaBridgeServerNames → injectModeCuaBrokerMcpServers 注入 broker 凭据
  └─ services 权威门（env && 启用）→ 自研 Helper 按需启动（demand 边界）
```

## clean-room 取材清单（允许输入）

- 本仓 spec：`computer-use-windows-runtime.md` 工具面/错误码节、`computer-use-enablement.md`；
- 接口面：`packages/mode-cua/index.js`（方法名/错误码常量与注释契约）、
  `node-repl-host/src/cua-bridge.ts`（bridge 接口类型）、宿主 seed/manifest 解析代码；
- 仓内 skill 格式（bundled-skills frontmatter 形状）；`DESIGN.md` 不涉及。
- 不读取上游 `zai-org/ZCode` 与本仓上游衍生实现；官方 docs/SKILL/SDK 不在本仓，无逐字来源。

## 验收场景

- [x] producer 包布局、manifest（name/version/无 mcpServers）、三个种子文件齐备。（单测 + seed 冒烟）
- [x] docs 以反引号覆盖 14 个方法名；client 导出同名 14 常量；SKILL frontmatter name=computer-use。（单测）
- [x] 两份 staging 清单含 `mode-cua-plugin` 条目；远端 requiredPaths 含其 manifest。（单测）
- [x] definitions 的 requiredSeedPaths 字符串与实际文件机械对照通过（单测）。
- [x] `pnpm install` 后 lockfile 含新 importer；typecheck / lint / 架构 / 语言政策全绿。
- [ ] 实机（待用户）：设置页开开关 → 插件可启用、node_repl 注册、Helper 按需启动、skill 可用。

## 非目标

- 不实现 macOS CUA（二期，docs/SKILL 明示 fail-closed）。
- 不为插件声明独立 MCP server；不携带官方 native/koffi 资产（`runtimeTopLevelPaths: []` 维持）。
- 不改宿主 seed/发现/注入代码；不改插件市场条目。
