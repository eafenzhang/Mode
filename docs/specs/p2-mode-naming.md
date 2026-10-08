# Spec：P2 命名统一（ZCode / ZCODIUM → Mode）

前置：`p0-user-facing-naming.md`（P0：bin / deep link / i18n / README）与
`p1a-external-env-renames.md`（P1a：对外环境变量 `ZCODE_` → `ZCODIUM_`）已完成。
本 spec 是 P1a 结尾写明的「P2：转独立维护时处理」那一步，并把 `ZCODIUM_` 再推进到
`MODE_`、把第一方包名与产物名统一到 Mode。

## 范围

| 面 | 现在 | 改成 | 兼容策略 |
| --- | --- | --- | --- |
| 对外环境变量 | `ZCODIUM_*`（旧名 `ZCODE_*`） | `MODE_*` | 三段链：读 `MODE_` → `ZCODIUM_` → `ZCODE_`；写全名 |
| 内部进程间变量 | `ZCODE_*`（约 330 个） | `MODE_*` | 无（进程内自洽） |
| 包名 | `@mode/*`（29 个包） | `@mode/*` | 无（同一提交原子切换） |
| 目录名 | `apps/mode-cli`、`packages/mode-*`、`scripts/mode-distribution`、`src/mode-protocol*`、`src/mode-{agent,session}` | `mode-*` | 无 |
| 产物名 | `dist/mode.cjs`、`mode-<版本>.tar.gz` | `dist/mode.cjs`、`mode-<版本>.tar.gz` | 无（安装器与文档同步） |
| 命令名 | `mode` | `mode` | 无 alias（P0 已定过同一口径） |
| 用户数据根 | `{base}/.mode` | `{base}/.mode` | 多代迁移：`.mode` ← `.mode` ← `.mode`，迁移失败保留旧路径 |
| Electron userData | `Mode` | `Mode` | 首次启动迁移旧目录，失败则继续用旧目录 |
| 插件市场 id | `zcode-plugins-official` | `mode-plugins-official` | 落盘数据迁移（设置 / 安装记录 / cache 目录） |
| 插件清单目录 | `.zcode-plugin/plugin.json` | `.mode-plugin/plugin.json` | 新名为主，**继续读旧路径**，第三方插件不作废 |
| 忽略文件 | `.zcodeignore` | `.modeignore` | 读新名，兼容旧名 |
| 项目级配置目录 | `.mode/` | `.mode/` | 读新名，兼容旧名 |
| 协议名 | `ZCode Protocol` | `Mode Protocol` | 服务端**同时接受**旧名（SSH 远端旧 agent 混布） |
| LAN cookie | `mode_lite_token` | `mode_lite_token` | 读旧 cookie 兜底 |
| MCP meta 前缀 | `com.mode/*` | `com.mode/*` | 读旧 key 兜底 + 迁移已存认证 |
| deep link | `mode://` | `mode://` | 注册新 scheme，并处理旧 scheme 链接；不注册旧 scheme（避免与宿主 ZCode 争抢） |
| 对外标识 | `WECOM_QR_SOURCE`、HTTP `appName` | `mode` / `Mode` | 企业微信侧实机验证，平台拒绝则回退 |
| 版权主体 | `Mode contributors` | `Mode` | 只改第一方 LICENSE，上游署名不动 |

## 必须保留（不得改动）

- `LICENSE-APACHE` 与 `scripts/license-texts/Apache-2.0.txt` 的 Z.AI / 占位符署名（Apache-2.0 §4）。
- `NOTICE.md` / `NOTICE.zh-CN.md` 的衍生作品段落（`zai-org/ZCode` 链接、Apache-2.0 说明、"版权与署名声明原样保留"），它是仓库级 §4(b) 修改声明的法律机制本体。
- `THIRD-PARTY-NOTICES.md` 与 `third-party/**` 的全部第三方原文与哈希台账。
- `packages/ui/src/components/ai-elements/*` 头部 Vercel 版权与上游修改标注。
- `.github/workflows/upstream-audit.yml` 里的真实远端仓库名 `ZCodium-project/ZCodium`（功能依赖，非品牌）。
- `.zcode-plugin` / `.claude-plugin` / `.codex-plugin` 的**读取兼容**（旧路径仍可被发现）。

## 验收

1. `pnpm typecheck`、`pnpm lint`、services / desktop / ui 测试、`architecture:check`、`knip` 全绿。
2. dev 实例：旧数据目录自动迁移后设置 / 机器人 / 凭证仍在；插件商店两段正常；已装插件仍启用。
3. 本地打一次 Windows 安装包：产物名、图标、安装器文案均为 Mode；改名后 CLI 产物可安装可运行。
4. 保留项逐条核对未被改动（`git diff` 检查上述文件）。
5. 第三方台账：`packages/desktop/package.json` 等台账输入变更后，在 CI（Linux）用
   `licenses-notices` workflow_dispatch 重生成并入库。

## 执行进度（截至本次提交）

已完成并全部通过门禁（typecheck 0 错误、lint 78 警告 0 错误、services 198 中 190 pass、
desktop 25/25、architecture 0/0/0）：

- **S1** 环境变量三段链 `MODE_` → `ZCODIUM_` → `ZCODE_`（`packages/shared/src/env-names.ts`
  的 `EXTERNAL_ENV_LEGACY_ALIASES`，读逐级回退、写三代全写）；内部 ~330 个 `ZCODE_*` 改名；
  内置 provider 配置 `config/provider/mode-builtin.json`（含 SEA 资产键 `mode-provider/*`）。
- **S2a** 29 个包 `@zcode/*` → `@mode/*`；**S2b** 目录与标识符（`apps/mode-cli`、
  `packages/mode-{cua,server-cli}`、`scripts/mode-distribution`、`src/mode-protocol{,-v4}`、
  `src/mode-{agent,session}`；`ZCode/Zcode/zcode` 各形态），内置 agent 产物 `mode.cjs`。
- **S3** CLI 命令名 `mode`、`bin.mode`、发行包 `mode-<版本>.tar.gz`。
- **S4** 数据根 `.mode` + 归属文件 `.mode-root.json` + 多代旧根（`.zcodium`、`.zcode`）；
  `isAcceptedDataRootProduct` 接受历史 product id（关键：避免存量根被判 unowned）；
  91 个文件的路径字面量；Electron userData 改名带一次性复制迁移（失败继续用旧目录）；
  i18n 迁移提示重写为真实旧目录名。
- **S5a** 清单目录 `.mode-plugin` 与忽略文件 `.modeignore` 新名+旧名兼容读取。
- **S5b** 随包插件（browser-use、node-repl-host）清单目录 git mv 到 `.mode-plugin`；
  services 三处清单探测、plugin-sync 候选、server 远端资产白名单同步为双名兼容。
- **S5c** 市场 id 切到 `mode-plugins-official`（详见下方"S5c 落地记录"）。
- **S6** 协议名、LAN cookie、MCP `_meta`、deep link 的"改名 + 接受旧值"（详见下方"S6 落地记录"）。
- **S7** LICENSE 版权主体 `Copyright (c) 2026 Mode`；maintainer、反馈链接、DMG 说明改 Mode。
  台账输入已变更，需在 CI（Linux）用 `licenses-notices` workflow_dispatch 重生成后入库。

## S5c 落地记录（已完成）

1. **常量切换**：`MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID`（shared）与
   `MODE_OFFICIAL_PLUGIN_MARKETPLACE`（contracts）取值改为 `mode-plugins-official`；
   旧值保留为 `*_LEGACY_ID`，`isOfficialMarketplaceId` / `isPublicStoreMarketplaceId` 同时接受。
2. **清单名覆盖**：随包快照（`official-plugin-catalog.generated.ts`）与
   `writeOfficialMarketplace` 分片写新 id；`assertOfficialManifest` 接受两个 id（CDN 清单
   声明的仍是旧名）；`rebuildOfficialMarketplaceSync` 的 canonical 名取新 id；
   `scripts/bundle-official-plugin-catalog.mjs` 与 `fetch-plugin-icons-by-name.mjs`
   优先读新目录名、回退旧目录名并强制写出新 id（Sea 资产键与 manifest 的 marketplace 同步）。
3. **插件 id 归一**：shared 与 contracts 各提供 `canonicalPluginId`（`…@zcode-plugins-official`
   → `…@mode-plugins-official`）；adapters 的 `canonicalizePluginId` 归一"市场段 + mode-cua 旧名"，
   `pluginIdAliases` 覆盖四种存量写法。读时归一点：`parseConfigFileToRuntimePatchWithDiagnostics`
   （enabledPlugins / options 键 + suppressedBuiltins 值）、`listInstalledPluginRecords`、
   services 的三个 `readPluginConfigFromConfig`/`readInstalledPluginRoots`/`readPluginRecords`。
   写路径（enable / options / suppression / 安装启用默认）一律落新 id；落盘迁移仍只针对
   `mode-cua@…` 旧名，不动市场段（不直接改写用户数据）。
4. **目录兜底**：`getPluginCacheDir`（cache）、`loadMarketplaceManifestSync`（marketplaces 清单）、
   `getPluginDataDir`（data）、`resolveOfficialPluginCacheRoot`（settings 侧扫 cache 共用）
   统一"新目录不存在 → 沿用旧目录"；`scanOfficialCache`（adapter 发现链）同口径。
5. **图标与 UI 字面量**：随包图标目录已是 `packages/ui/src/assets/plugin-icons/mode-plugins-official`，
   `resolvePluginIconSource` 查找前先归一 id；官方插件 id 字面量（mcp.ts 的 CUA id、
   BrowserSettingsSection、pluginCreatorPrefill、featureSuggestedPrompts 的 plugin:// 链接、
   builtinSkillI18n 的路径标记）全部切新 id 或双认。
6. **验证**：新增 `packages/services/test/plugin-marketplace-id-compat.test.ts`（id 归一、
   缓存根回退、协议名双认、`_meta` 双读）；adapters 侧读时归一/写时新 id 用一次性 tsx 脚本
   逐条断言（canonicalizePluginId、键冲突优先级、旧键清理、suppression 迁移）。
7. **随包插件清单目录双认（连带修复）**：S5b 把随包插件清单改到 `.mode-plugin`，但 seed 链与
   构建脚本仍在找 `.zcode-plugin`，导致 seed 源解析不到任何插件（随包插件不再 seed、官方市场
   分片停在旧内容、SEA/远端资源准备会报缺清单）。已统一为"新名优先、旧名兜底"：
   `bundled-plugins.ts`（顶层白名单 / 根探测 / description / isSeedUsable）、
   `official-plugin-runtime.ts`（`resolveOfficialPluginManifestPath`）、
   `zip-source.ts`（zip 插件清单探测）、`subagentsService.ts`（插件子代理清单路径）、
   `scripts/prepare-prebuilds.mjs`、`packages/desktop/scripts/prepare-agent-node-bundle.mjs`、
   `apps/mode-cli/packages/cli/scripts/sea-official-plugin-assets.mjs`。

## S6 落地记录（已完成）

- **协议名**：`MODE_PROTOCOL_NAME = "Mode Protocol"`；新增 `MODE_PROTOCOL_LEGACY_NAME`
  与 `MODE_PROTOCOL_ACCEPTED_NAMES`，`modeSessionStateSnapshotSchema` 用
  `z.enum(MODE_PROTOCOL_ACCEPTED_NAMES)` 校验（远端旧 agent 的 snapshot 仍可握手），
  写入与回显一律新名。单测覆盖"旧名接受、未知名拒绝"。
- **LAN cookie**：服务端读 `mode_lite_token` 优先、`zcode_lite_token` 兜底（老客户端不用重新配对），
  `Set-Cookie` 只写新名。新增 `packages/server/test/lan-cookie-compat.test.ts` 用真实 HTTP 服务
  验证四态（无令牌 401 / 旧 cookie 200 / 新 cookie 200 / 错误令牌 401），并已接入 release.yml
  的 verify 闸门（`pnpm --filter @mode/server test`）。
- **MCP `_meta`**：命名空间已是 `com.mode/*`；读取侧（node-repl-host 的
  `com.mode/request-context`）新增旧键 `com.zcode/request-context` 兜底，
  生产侧改用共享常量 `MODE_MCP_REQUEST_CONTEXT_META_KEY`。身份头键
  `com.mode/official-mcp-auth` 的读取方是官方插件的 server 进程（仓库外），
  故只记录历史值、不做双读。
- **deep link**：只注册 `mode`（未注册旧 scheme）；`desktopDeepLinkUrl.ts` 的提取正则与
  四个回调判据（oauth / payment / workspace / share）同时接受 `zcode://`，
  新增 `packages/desktop/tests/deep-link-scheme-compat.test.mjs` 钉住。
- **对外标识**：`WECOM_QR_SOURCE = "mode"`、HTTP 客户端 `appName = "Mode"` 均已是新值
  （无改动）。企业微信二维码是否被平台接受需要实机建立一次二维码，属于 S8 的实机项。

## S8 验收记录（2026-10-08）

**门禁（全绿）**：`pnpm typecheck` 0 错误（含 mode-cli 的 contracts/adapters/bootstrap/node-repl-host）；
`pnpm lint` 77 警告 0 错误（比基线少 1 条：清掉 `server/src/http.ts` 里改名遗留的未用 import）；
services 203 测试（195 pass / 8 skip，含新增 5 条）、desktop 29（25 + 新增 4 条 deep link）、
server 1（新增 LAN cookie）、ui 18；`architecture:check --changed` 0 violations。
新增 `pnpm --filter @mode/server test` 已接入 release.yml 的 verify 闸门。

**实机（dev 实例 + CDP）**：
- 插件商店两段正常：公开 25 张卡、个人 315 张卡（Claude 目录），第三方条目状态未受影响。
- 切换后首次启动生成了 `marketplaces/mode-plugins-official/{marketplace.json,bundled-marketplace.json}`
  （`name = mode-plugins-official`，40 条目）与 `cache/mode-plugins-official/{browser-use/0.5.1,node-repl-host/0.6.0}`
  （旧根 `cache/zcode-plugins-official/**` 保持可用，走兜底读取）。
- `mode plugins list`（bundled CLI）输出 `browser-use@mode-plugins-official [enabled]`、
  `node-repl-host@mode-plugins-official [enabled]`（来自新缓存根）、
  `activecampaign@claude-plugins-official [enabled]`（第三方不受影响）；
  `~/.mode/cli/config.json` 的 enabledPlugins 已是新 id。
- 数据迁移：`MODE_DATA_BASE_DIR=<old dev home> MODE_DATA_ROOT_ACTION=migrate` 跑一次 CLI，
  旧根 `.zcodium` 被识别为历史产品并迁移到 `.mode`（`.mode-root.json` 记录 migration.from），
  机器人配置、凭证（bot credential、LAN 访问令牌）逐项仍在，旧根按 copy 语义保留。
- 未覆盖（如实记录）：企业微信二维码 source=mode 的平台接受度、Windows 安装包与 CLI 产物的
  本地冒烟、`licenses-notices` 台账重生成（CI/Linux），以及桌面端数据根迁移对话框的点击路径。

## S5c/S6 的原始步骤（参考）

### S5c 市场 id `zcode-plugins-official` → `mode-plugins-official`

要求：**改名 + 旧 id 全程兼容**，否则已装插件会被判为未安装。落地顺序：

1. 常量换新名并保留旧名：`packages/shared/src/plugin-marketplaces.ts` 与
   `@mode/contracts` 的官方市场 id、`isOfficialMarketplaceId` 同时接受旧 id。
2. 清单名覆盖：随包快照（`apps/mode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts`，
   由 `scripts/bundle-official-plugin-catalog.mjs` 生成）的 `name` 与
   `writeOfficialMarketplace` 分片写的 name 都用新 id；`assertOfficialManifest`
   同时接受 CDN 清单声明的旧 name（官方清单本身改不了）。
3. 插件 id 归一：新增 `canonicalPluginId(id)`，把 `…@zcode-plugins-official` 映射成
   `…@mode-plugins-official`，在用户设置的读写边界（`enabledPlugins`、`suppressedBuiltins`、
   `installed_plugins.json`、`config.plugins.*`）统一归一——**不要**直接改写用户数据，
   读时归一、写时用新 id 即可平滑过渡。
4. 缓存目录兜底：`getPluginCacheDir` 解析时若新目录不存在而旧目录（`cache/zcode-plugins-official/**`）
   存在则用旧目录；这样已装插件的文件原地可用。
5. 资源与生成物：图标资源目录 `packages/ui/src/assets/plugin-icons/<新 id>` 与
   `modeOfficialPluginIcons.generated.ts` 的键、`scripts/fetch-plugin-icons-by-name.mjs` 同步。
6. 验收：实机确认 browser-use / node-repl-host 仍启用、商店「公开/个人」两段正常、
   已装插件可启停与卸载、第三方（Claude）目录不受影响。

### S6 协议与对外标识（一律"改名 + 接受旧值"）

- `MODE_PROTOCOL_NAME` 的值改 `"Mode Protocol"`，握手校验同时接受 `"ZCode Protocol"`（SSH 远端旧 agent 混布）。
- LAN cookie `zcode_lite_token` → `mode_lite_token`，服务端读旧 cookie 兜底；`scripts/verify-lan-attach.mjs` 同步。
- MCP `_meta` 前缀 `com.zcode/*` → `com.mode/*`，读旧 key 兜底（`packages/shared/src/official-mcp-auth.ts` 等）。
- deep link：`DEEP_LINK_SCHEME` 已是 `mode`（S2b 顺带改），补"仍处理 `zcode://` 传入链接、不主动注册旧 scheme"。
- `WECOM_QR_SOURCE` 改 `mode`（实机建一次二维码验证平台接受，不接受则回退）；HTTP 客户端 `appName` 改 `Mode`。

### S8 验收

- 全量门禁 + 实机：数据迁移后设置/机器人/凭证仍在、插件商店两段、机器人、局域网连接。
- 本地打一次 Windows 安装包，验产物名、图标、安装器文案；改名后的 CLI 产物跑冒烟。
- 小尾巴：dev 启动脚本里内置配置旧前缀剔除待核对；lint 多的 1 条 warning（无 error）。
