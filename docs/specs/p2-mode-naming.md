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
- **S7** LICENSE 版权主体 `Copyright (c) 2026 Mode`；maintainer、反馈链接、DMG 说明改 Mode。
  台账输入已变更，需在 CI（Linux）用 `licenses-notices` workflow_dispatch 重生成后入库。

## 剩余工作（S5c / S6 / S8）

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
