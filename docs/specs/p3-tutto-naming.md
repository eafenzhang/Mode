# Spec：P3 全仓库改名 Mode → Tutto

用户裁决（2026-10-19）：**全仓库内部重命名**；图标采用**几何字母 T**。

## 范围

| 面 | 现在 | 改成 | 兼容策略 |
| --- | --- | --- | --- |
| 包名 | `@mode/*`（29 包） | `@tutto/*` | 无（同一原子提交切换 import/tsconfig/构建） |
| 目录 | `apps/mode-cli`、`packages/mode-*`、`scripts/mode-*`、`src/mode-protocol*` | `tutto-*` | 无 |
| 对外环境变量 | `MODE_*`（含 zcode/zcodium 历史链） | `TUTTO_*` | 读 `TUTTO_` → `MODE_` → 旧链；写全名 |
| 用户数据根 | `{base}/.mode` | `{base}/.tutto` | 一次性迁移 `.mode` → `.tutto`，失败保留旧根；接受历史根 |
| Electron userData | `Mode` | `Tutto` | 首启复制迁移旧目录，失败继续用旧目录 |
| 协议名 | `Mode Protocol` | `Tutto Protocol` | 握手同时接受 `Mode Protocol` 与更早名字 |
| LAN cookie | `mode_lite_token` | `tutto_lite_token` | 读旧 cookie 兜底，写新名 |
| MCP meta | `com.mode/*` | `com.tutto/*` | 读旧 key 兜底 |
| deep link | `mode://` | `tutto://` | 注册新 scheme，继续处理旧 `mode://`；不注册旧 scheme |
| 插件市场 id | `mode-plugins-official` | `tutto-plugins-official` | 旧 id 全程兼容 + 读时归一 + 写新 id |
| 插件清单目录 | `.mode-plugin` | `.tutto-plugin` | 新名优先、旧名兜底 |
| 忽略文件 | `.modeignore` | `.tuttoignore` | 读新名，兼容旧名 |
| 应用身份 | `dev.mode.app` / `Mode` | `dev.tutto.app` / `Tutto` | **破坏性**：新旧版可并排安装；升级需数据迁移（userData/data root） |
| 图标 | 黑底 M 渐变 | 黑底几何 T 渐变 | 重新生成 PNG/ICO/ICNS |
| 文档/工作流产物名 | `Mode-<版本>-...` | `Tutto-<版本>-...` | 发版公告说明新产物名 |

## 必须保留

- Apache/NOTICE/THIRD-PARTY 与上游署名、上游链接 `zai-org/ZCode`；
- 第三方台账原文与哈希；
- 旧路径/旧 id/旧 cookie/旧协议名的**读取兼容**（不得直接改写用户数据）；
- `packages/mode-cua/**` 的并行会话归属：本 spec 不改该目录，等并行会话收口后另行切换。

## 分阶段执行

- **S1 可见身份与图标**（本轮）：productName/窗口标题/安装产物名、electron identity、Tauri 配置、应用图标、About logo、核心 i18n 产品名、release 产物名。
- **S2 包作用域**：`@mode/*` → `@tutto/*`，import/tsconfig/构建/锁文件原子切换。
- **S3 环境变量与数据兼容**：`TUTTO_*` 写入 + `MODE_*` 读取；`.tutto` 数据根迁移；userData 迁移。
- **S4 协议与生态标识**：deep link、cookie、MCP meta、市场 id、插件目录、协议名。
- **S5 目录与文档**：`apps/mode-cli`、`packages/mode-*`、`scripts/mode-*`、`src/mode-*` 改名；README/NOTICE 指针与路线图。

## 验收

1. 每阶段 `pnpm typecheck`、`pnpm lint`、相关包测试、`pnpm architecture:check --changed` 全绿；
2. S3 后：旧 `.mode` 数据根与旧 userData 能迁移到 Tutto，设置/机器人/凭证/插件仍在；
3. S4 后：旧 deep link/cookie/协议/市场 id 仍可读；
4. 本地打 Windows 安装包：产物名、图标、安装器文案为 Tutto；
5. 保留项逐条 `git diff` 核对未改。

## 执行进度

### S1 可见身份与图标（2026-10-19 完成）

- `productName` / Tauri `identifier`（`dev.tutto.app`）/ Electron identity（正式 `Tutto`、预览
  `Tutto Preview`、Linux 包名 `tutto*`）/ Tauri 窗口标题改为 Tutto；
- 应用图标更换为几何 T（薄荷→蔚蓝渐变、黑底 squircle、叠层 T），重新生成
  PNG/ICO/ICNS 并同步 `apps/desktop-tauri/src-tauri/icons`、`packages/desktop/build/{icon*,icons/*}`；
- About/侧栏品牌标记换成 `TuttoLogo`；三语 i18n 产品名、HTML 标题、强制更新页、
  扩展显示名、release 产物名（`Tutto-<版本>-…`）与发布说明同步；
- Electron userData 改名 `Tutto`，并做 `Tutto ← Mode ← ZCodium` 的首启复制迁移；
- 门禁：typecheck 0、UI 391 tests 全绿、UI lint 0 error、architecture 0、语言政策通过。
- **未做（后续阶段）**：`@mode/*` 包名与目录、`MODE_*` 环境变量与 `.mode` 数据根/协议/cookie/
  市场 id/插件目录/deep link 的切换及兼容；CLI 命令名与二进制名。

### S4 进展（2026-10-19，deep link 完成）

- 新 scheme `tutto://` 为默认；识别链为 `tutto://` → `mode://` → `zcode://`；
- Tauri deep-link 配置、Electron `setAsDefaultProtocolClient`、Linux `.desktop`
  `MimeType` 同时注册 `tutto` 与 `mode`；
- Finder 工作区工作流改用 `tutto://`；
- **例外**：外部 OAuth 服务登记的 redirectUri 仍是 `mode://oauth/callback`，
  在服务端完成新 URI 登记前不改，避免登录回调失效；
- 测试：`packages/desktop/tests/deep-link-scheme-compat.test.mjs` 4/4 通过
  （tutto/mode/zcode 接受、https 与非完全匹配的 scheme 拒绝、三种 scheme 解析 path）。
- 仍待做：cookie、MCP meta、市场 id、插件目录、协议名。

### S4 进展（2026-10-19，LAN cookie 完成）

- 新 cookie 名 `tutto_lite_token`；读取链 `tutto_lite_token` → `mode_lite_token` → `zcode_lite_token`，
  `Set-Cookie` 只写新名；
- 桌面 LAN attach、`verify-lan-attach`、fnOS 网关 proxy 同步发送新名；
- 测试 `packages/server/test/lan-cookie-compat.test.ts` 覆盖无令牌 401、三代 cookie 放行、
  只下发新名、错误令牌 401；server 测试 30/30 通过。

### S4 进展（2026-10-19，协议名与 MCP request-context）

- 协议名 `Tutto Protocol`；握手接受 `Tutto Protocol` → `Mode Protocol` → `ZCode Protocol`，
  写入/回显只用新名；
- MCP request-context 键 `com.tutto/request-context`；读取兜底
  `com.mode/request-context`、`com.zcode/request-context`；
- **例外**：`OFFICIAL_MCP_AUTH_META_KEY` 保持 `com.mode/official-mcp-auth`——读取方是
  已发布插件的 server 进程，改名会破坏全部已装插件（spec 记录为外部契约例外）；
- `plugin-marketplace-id-compat.test.ts` 5/5 通过（协议名接受/拒绝、request-context 双旧键）。
