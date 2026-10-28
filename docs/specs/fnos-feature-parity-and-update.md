# Spec：fnOS fpk 功能补齐与在线升级

状态：2026-10-27 立项。关联：
[fnos-fpk-packaging.md](./fnos-fpk-packaging.md)（包结构与生命周期）、
[remote-control-device-model.md](./remote-control-device-model.md)（Web/手机远控）、
[unified-frontend-build.md](./unified-frontend-build.md)（Web 前端）、
[legacy-electron-eol.md](./legacy-electron-eol.md)（桌面能力边界）。

## 1. 背景

fnOS 版是 Web 形态（`mode --web` + 浏览器 UI），不是桌面 GUI。当前 fpk 已经能安装、
启动、通过飞牛统一网关打开，并使用任务、插件、终端、文件树等由 Host 提供的服务；
但 `packages/web/src/main.tsx` 的 `createWebPlatform()` 仍把一批平台能力做成**桩**，
用户在 NAS 上会看到「入口在、点了没反应」或「入口直接不渲染」。同时，fpk 的升级
过去只有「应用中心手动安装新 fpk」一条路，没有应用内检查/下载/升级。

本 spec 先固定两件事：

1. 把 fpk 当前的功能缺口按「平台边界」列清楚，区分**可实现**与**不适用**，
   作为后续阶段排期的唯一清单；
2. 定义 **P0：fpk 在线升级** 的产品规则、所有者、协议、失败回滚与验收。

## 2. 现状与平台边界（证据）

| 能力 | 当前实现（`packages/web/src/main.tsx`） | 判定 |
| --- | --- | --- |
| 本地工作区打开/创建 | `preferDirectoryBrowser` + 服务端 `fileService`；创建工作区走 `createScratchWorkspace` | ✅ 可用 |
| 终端 / 文件树 / Git / 任务 / 插件 / 自动化 | 服务端 `createLocalServices` 完整暴露在 `/ws` | ✅ 可用 |
| 浏览器附件 | composer 的隐藏 `<input type=file>` + Web bytes 上传（`useComposerAttachments`） | ✅ 可用 |
| 浏览器通知 | `window.Notification` | ✅ 可用（需用户授权） |
| 帮助菜单（关于/导出日志） | Web 端渲染「导出日志 / 关于」；资源管理器与桌面更新器仍隐藏 | ✅ 2026-10-27 实现 |
| 导出日志 | Web 平台走 `GET /api/logs/export` 下载文本包（Host 日志 + fpk `info.log`） | ✅ 2026-10-27 实现 |
| 在线升级 | `/api/app-update/*` + 设置页「应用更新」卡片 + `cmd/main restart`/回滚 | ✅ 2026-10-27 实现 |
| MCP 用户目录管理 | MCP store 在 `supportsLocalMcpDirectory=false` 时走 Host `mcpSyncService`，平台桩不阻塞常规读写 | ✅ 常规路径可用（原生目录管理仍不适用） |
| 远程工作区（SSH/LAN） | `connectRemote` 桩返回 unsupported；`Root` 收到 `allowRemoteWorkspace=false`；`/ws/remote/:id` 只注册 file/git/system/terminal 四个服务 | ⚠️ 部分（需要完整 service accessor，单独立项） |
| 远程桌面（作为主控） | `rdStartSession` 走 NAS 中继，浏览器复用 `nativeMediaSession`（WebCodecs + WebCrypto） | ✅ 2026-10-28 实现（真机画面/输入待验收） |
| 局域网配对（作为主控） | `pairLanPeer`/`getLanPeerWorkspaces` 等桩拒绝 | ⚠️ 可做，依赖远程工作区/设备模型 |
| 打开本机文件管理器 / 编辑器 | `openInFileManager`/`getInstalledEditors` 桩 | ❌ 不适用（NAS 无桌面） |
| Chrome 浏览器数据导入 / 清除 | 桩拒绝 | ❌ 不适用（无本机 Chrome） |
| CUA（电脑操作） | Windows-only | ❌ 不适用（Linux NAS） |

**结论**：用户感知的「功能缺少很多」主要是 **帮助/日志/更新** 与 **远程工作区/远程
桌面** 两类。前者已在 2026-10-27 完成（见 §5）；后者依赖浏览器媒体管线与完整远程
service 面，不在本轮范围。

## 3. P0：fpk 在线升级

### 3.1 产品规则

- 只在 **fpk 部署**启用：`cmd/main` 启动 runner 时注入 `MODE_FNOS_UPDATE=1`、
  `MODE_FNOS_APPDEST`、`MODE_FNOS_MAIN_SCRIPT`，并把 `TRIM_APPDEST`、`TRIM_PKGVAR`、
  `TRIM_PKGETC`、`TRIM_PKGHOME`、`TRIM_SERVICE_PORT`、`TRIM_DATA_ACCESSIBLE_PATHS`、
  `TRIM_USERNAME`、`TRIM_TEMP_LOGFILE` 一并传给 runner，供重启命令复用同一生命周期
  脚本。普通 `mode --web` / 独立 Web 部署的端点返回 `{ enabled: false }`，不显示更新入口。
- 只升级**载荷**（`$TRIM_APPDEST` 下的 `bin/`、`web/`、`server/`、`agent/`、
  `node_modules/`、`runtime/`、`proxy.mjs`、`ui/`），不修改 fpk 外壳
  （`manifest`、`cmd/`、应用中心记录）。外壳升级仍走应用中心安装新 fpk。
- 版本比较只接受**正式版**（`x.y.z`），忽略预发布后缀；不使用降级。
- 更新源固定为 GitHub Releases：
  - 版本/说明：`https://github.com/eafenzhang/Mode/releases/latest/download/latest.json`
  - 校验清单：同 release 的 `sha256.txt`
  - 安装包：`https://github.com/eafenzhang/Mode/releases/download/v<version>/mode-<version>-x86_64.fpk`
  - 允许 `MODE_FNOS_UPDATE_BASE_URL` 覆盖（测试/镜像用），默认上述地址。
- 下载后必须校验 `sha256.txt` 中对应 `.fpk` 的哈希；校验失败不落盘为可应用状态。
- 版本一致性：解包后读取 fpk 外壳 `manifest` 的 `version` 与载荷 `package.json` 的
  `version`，两者都必须等于 `latest.json` 声明的版本；任一不符按 `version_mismatch`
  拒绝（外壳版本进应用中心记录，载荷版本进运行时 `MODE_VERSION`，错位会让两处显示不同版本）。
- 可选发布签名：`$TRIM_PKGETC/update-public-key` 存在时，`cmd/main` 把内容注入
  `MODE_FNOS_UPDATE_PUBLIC_KEY`，服务端要求同一 release 的 `sha256.txt.sig` 通过
  Ed25519 验签后才接受哈希清单。公钥缺失 = 不启用（维持 TLS + sha256 信任模型）；
  公钥存在但无法解析、签名缺失或不匹配一律拒绝下载，不静默降级。
- 应用前必须通过结构检查：`bin/mode.mjs`、`web/index.html`、`server/`、`agent/`、
  `node_modules/`、`runtime/bin/node`、`proxy.mjs`、`ui/config` 均存在。
- 升级成功后由 `cmd/main restart` 重启服务；重启失败自动回滚到旧载荷并再次尝试启动。
- **首次引导**：当前已发布的 fpk（<=0.0.94）的 `cmd/main` 没有 `restart` 与更新环境
  变量，因此必须在应用中心手动安装一次带本功能的 fpk；之后的版本才能应用内升级。

### 3.2 状态所有者

| 状态 | 唯一所有者 | 位置 |
| --- | --- | --- |
| 更新状态（phase/进度/错误/版本） | 服务端 `FnosAppUpdater` 单例（进程内） | 内存；不持久化 |
| 当前版本 | `MODE_VERSION`（发行产物） | 只读 |
| 下载包与解包载荷 | `FnosAppUpdater` | `$TRIM_APPDEST/.update/` |
| 服务进程 / 代理进程 | `cmd/main`（不变） | `$TRIM_PKGVAR/app.pid`、`proxy.pid` |
| 浏览器投影 | Web 平台方法 + 设置页卡片 | 只读 + 命令，不保存第二份真相 |

### 3.3 协议（`/api/app-update/*`，受既有 `/api/*` 令牌鉴权保护）

```ts
type FnosAppUpdatePhase =
  | "idle" | "checking" | "up-to-date" | "available"
  | "downloading" | "ready" | "applying" | "error";

interface FnosAppUpdateState {
  enabled: boolean;
  currentVersion: string;
  phase: FnosAppUpdatePhase;
  latestVersion?: string;
  progress?: number;       // 0..100
  error?: string;          // 稳定错误码或可读文本
  checkedAt?: string;      // ISO
  canApply: boolean;
}

GET  /api/app-update/state    -> FnosAppUpdateState
POST /api/app-update/check    -> FnosAppUpdateState   // 同步检查 latest.json
POST /api/app-update/download -> FnosAppUpdateState   // 202 语义：立即返回，后台下载
POST /api/app-update/apply    -> { ok: boolean; restarting: boolean; error?: string }
```

- `download` 在后台执行：下载 `.fpk` → 校验哈希 → 解外层 tar.gz → 解 `app.tgz` →
  结构检查 → 版本一致性检查。失败回到 `error`，保留旧载荷。
- 配置公钥时额外拉取 `https://github.com/eafenzhang/Mode/releases/download/v<version>/sha256.txt.sig`
  （与 `sha256.txt` 同 release），验签通过才继续；未配置时不发这个请求。
- `apply` 只在 `ready` 允许；服务端完成载荷交换后写 `.update/in-progress` 标记，
  交换完成删除标记，随后 `spawn(bash "$MODE_FNOS_MAIN_SCRIPT" restart)`。
  响应先返回，连接随后被重启断开是预期行为。
- 同一时间只允许一个 check/download/apply；重复请求返回当前状态。

### 3.4 事件顺序

```text
用户点击「检查更新」
  → POST /check → GET latest.json → 版本比较
  → available / up-to-date / error（写状态）
用户点击「下载」
  → POST /download（202）→ 状态 downloading(0..100)
      → 下载 fpk → 校验 sha256.txt → 解包 → 结构检查
      → ready
用户点击「重启并更新」
  → POST /apply → 写 .update/in-progress
      → 逐项 rename：旧 → .update/backup，新 → $TRIM_APPDEST
      → 删除 in-progress
      → spawn cmd/main restart（detached）
  → cmd/main stop（SIGTERM 旧 runner + proxy）→ start
      → 启动失败且 .update/backup 存在 → 回滚旧载荷 → 再启动一次
      → 启动成功 → 清理 .update
  → 浏览器重连 /ws，读取新版本
```

### 3.5 失败语义

| 场景 | 行为 |
| --- | --- |
| 网络失败 / latest.json 非 2xx | `error`，保留当前版本；可重试 |
| 版本不更新或更旧 | `up-to-date`，不提供下载 |
| `sha256.txt` 缺条目或哈希不符 | `error`，删除下载包，不进入 ready |
| 公钥已配置但 `sha256.txt.sig` 缺失/不匹配 | `error: signature_missing` / `signature_invalid`，不下载 fpk |
| 公钥已配置但无法解析 | `error: invalid_public_key`，拒绝下载（不降级为无签名） |
| 外壳/载荷版本与 latest.json 不一致 | `error: version_mismatch:...`，删除解包目录 |
| 解包发现路径穿越 | `error`，删除解包目录（复用 `extractTarGzArchive` 的校验） |
| 磁盘空间不足 | 下载前预检，`error: insufficient_space` |
| 结构检查失败 | `error: invalid_payload`，删除解包目录 |
| 交换中途失败 | 立即从 backup 回滚；`error: apply_failed` |
| 重启后旧进程仍存活 | `cmd/main restart` 的 stop 负责 SIGTERM→10s→SIGKILL |
| 新载荷启动失败 | `cmd/main start` 检测到进程秒退，回滚 backup 后重试一次 |
| 应用中心/用户手动停服 | 不影响已 staged 的包；下次启动仍可 apply |
| 非 fpk 部署 | `enabled: false`，UI 不渲染入口 |

### 3.6 安全与资源边界

- 全链路 HTTPS；只接受 GitHub Releases 的 `latest.json` / `sha256.txt` 与其同 release
  的 `.fpk`。默认信任模型是 TLS + 发布流水线 + 同 release 哈希清单；运营方可在
  `etc/update-public-key` 放入 Ed25519 公钥（PEM 或 32 字节裸 base64）启用可信发布：
  此时 `sha256.txt.sig`（对 `sha256.txt` 原始字节的分离签名，base64）必须验证通过，
  否则拒绝下载。私钥只保存在发布流程中，不进仓库、不进安装包。
- 公钥未配置时行为与旧版一致，spec 明确记录「未声称签名校验」，不把可选能力写成默认已启用。
- 更新端点沿用 `/api/*` 令牌鉴权；未配对/无令牌请求 401。
- 解包复用 `packages/server/src/remote/localTarGz.ts` 的路径穿越/符号链接校验；
  解压后不执行任何包内脚本，只做结构检查。
- 峰值内存：外层 fpk + 内层 app.tgz 各解压一次，约 300–400 MB；下载前用 `statfs`
  要求至少 `4 × fpk 大小`（默认约 320 MB）可用空间（下载包 + app.tgz + 解包载荷 +
  余量；旧载荷备份是 rename，不额外占空间）。
- `.update` 目录不进入静态资源根，不被网关/LAN 静态服务暴露。

### 3.7 UI

- 设置 → 通用新增「应用更新」卡片，仅在 `platform.getFnosAppUpdateState` 存在时渲染
  （Web/fpk）；桌面端不渲染。
- 卡片显示当前版本、状态、下载进度、错误；按钮：检查更新 / 下载 / 重启并更新。
- 文案三套 locale；RTL 使用逻辑属性。更新过程中不阻塞其他设置。

### 3.8 验收

1. 非 fpk Web 部署：`/api/app-update/state` 返回 `enabled:false`，设置页无卡片。
2. fpk 部署 + 无新版本：检查后 `up-to-date`，无下载按钮。
3. 有新版本：`available` → 下载进度到 100% → `ready`；`sha256.txt` 哈希不符时
   停在 `error` 且不产生可应用包。
4. 应用后服务重启、`/api/server-info.version` 变为新版本；`.update/backup` 在成功
   启动后被清理。
5. 人为破坏新载荷的 `bin/mode.mjs`：重启失败后自动回滚，旧版本可继续访问。
6. 更新端点无令牌返回 401；`.update` 不通过静态路径可访问。
7. 单元测试覆盖版本比较、fpk 资产选择、sha256 解析、外壳/载荷版本一致性、可选签名、
   载荷结构检查与交换/回滚。
8. 配置公钥后：缺失/错误签名的发布包停在 `error`，正确签名可到 `ready`；未配置公钥时
   不请求 `.sig`、行为与旧版一致。

## 4. 后续阶段（不在 P0 实现）

| 优先级 | 事项 | 状态 |
| --- | --- | --- |
| P1 | 帮助菜单 + 导出日志 | ✅ 已完成（2026-10-27，§5） |
| P1 | MCP 用户目录管理 | ✅ 常规读写走 Host `mcpSyncService`；原生目录管理不适用 |
| P2 | 远程工作区（Web 作为主控） | 待做：`/ws/remote/:id` 扩到完整 service accessor；`allowRemoteWorkspace=true`；需单独 spec |
| P3 | 远程桌面（浏览器主控） | ✅ 已实现中继链路（2026-10-28，[web-remote-desktop.md](./web-remote-desktop.md)）；真机待验收 |
| 不适用 | CUA、本机编辑器、文件管理器、Chrome 导入 | 与 NAS/Linux 形态冲突，保持隐藏 |

## 5. P1：帮助菜单、日志导出与临时附件（2026-10-27 / 10-28）

- `GET /api/logs/export`（既有 `/api/*` 鉴权）：与桌面 `exportLogs` 同包格式；
  只选 `host-*.log` 最新 7 份，旧线/非 Host 文件进 `omitted` manifest。导出按
  「先 stat 选文件、再只读各自尾部」执行（单文件 5 MiB / 总量 20 MiB），
  NAS 上单个大日志不会整文件读入内存。
- fpk 的 `cmd/main` 通过 `MODE_FNOS_INFO_LOG` 把 `info.log` 交给服务端，作为额外
  文件随包导出；桌面 LAN 服务同样装配 `logsExport`，手机/浏览器的 Help 菜单可用。
- Web 平台 `exportLogs` 触发浏览器下载；`WorkspaceHelpMenuButton` 在 Web 端渲染
  「导出日志 / 关于」，资源管理器与桌面更新器继续隐藏（更新入口在设置 → 通用）。
- 关于对话框从 `/api/server-info.version` 取版本，不依赖桌面壳的 `ShowAbout`。
- 验收：无令牌 401；有令牌返回 `text/plain` 下载；包内含 Host 日志、`info.log`
  与 `omitted` manifest；无日志返回 404 `no_logs`。
- 长粘贴文本附件：Web 平台 `createTempTextAttachment` 改为
  `POST /api/attachments/temp-text`，写入 `{dataRoot}/.mode/v2/tmp/attachments/<唯一目录>/<文件名>`；
  文件名清洗规则与 Tauri 命令一致（只取最后一段、替换 Windows 非法字符、空名回退
  `attachment.txt`），上限 5 MiB。桌面端仍走原生命令。
