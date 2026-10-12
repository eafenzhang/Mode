# Spec：fnOS 原生应用（fpk）打包

状态：已实现（2026-10-12，首版构建脚本与骨架；飞牛真机安装验证待执行）。
关联：[mode-data-root.md](./mode-data-root.md)（数据根与无 UI 初始化）、[language-policy.md](./language-policy.md)（新增脚本扩展名豁免）。

## 背景与目标

- 目标：把 Mode 打包为飞牛 fnOS 的**原生应用**（`.fpk`，App Center 安装、桌面入口、启停控制），不是 Docker 应用。
- fnOS 是无显示服务的 NAS 系统，因此打包对象是 **Web 形态**（`mode --web`：静态 Web UI + HTTP/WS RPC + 按会话拉起 agent 子进程），Electron 桌面 GUI 不在范围内。
- 复用原则：直接消费 `scripts/build-mode.mjs` 的发行产物（web + server + agent），**不改动 Mode 源码**；fpk 专属内容（manifest、生命周期脚本、捆绑运行时）全部落在 `scripts/fnos-fpk/`。

## 产物与命名

| 项 | 约定 |
| --- | --- |
| 构建命令 | `node scripts/build-fnos-fpk.ts`（`--help` 见参数） |
| 产物 | `dist/fnos/releases/<version>/mode-<version>-x86_64.fpk` + `.sha256` |
| 版本默认 | 最近 tag（`git describe --tags --abbrev=0`，去 `v` 前缀）；`--version` 覆盖 |
| appname | `mode`；`platform = x86`（fnOS 当前主架构 x86_64） |
| 捆绑运行时 | 官方 Node `v24.14.0` linux-x64（放 `runtime/bin/node`，随附 `runtime/LICENSE`）。镜像与仓库既有约定一致：`MODE_NODE_DIST_MIRROR`，默认 `https://cdn.npmmirror.com/binaries/node`，回退 `https://nodejs.org/dist` |
| fnpack | 官方 `fnpack 1.2.3`；`MODE_FNPACK_PATH` 可指向本地二进制，否则按平台自动下载（win32-x64 校验 sha256） |

打包机要求：Linux 或 Windows 均可。仓库 `pnpm-workspace.yaml` 的 `supportedArchitectures` 已包含 linux x64/arm64，`@lydell/node-pty-linux-*` 预编译件在开发机上齐备，因此 Windows 也能产出完整 linux-x64 发行包。

## GitHub Release 集成

`release.yml` 的 `fnos` job 与桌面矩阵同一触发条件（推送自动发版 / 手工触发 `build_artifacts`），
在 ubuntu-latest 上执行 `node scripts/build-fnos-fpk.ts --version <release.version>`：

- 产物 `mode-<version>-x86_64.fpk` 经 `gh release upload --clobber`（3 次重试，与桌面腿一致）附到 Release。
- 校验：构建脚本生成的 `<fpk>.sha256` 作为 fragment 交给 checksums job 合并进 `sha256.txt`；
  合并清单的覆盖断言增加 `.fpk`（缺任一类即失败）。
- Release 说明增加 fnOS 安装条目（中英双语），文件名与实际产物一致。
- publish 门槛加入 fnos 结果：与桌面/校验清单同级，任一失败保持 draft。
- fnpack 与 Node 运行时的下载缓存在 `dist/fnos/.cache`（cache key 含 `build-lib.ts` 哈希，版本常量变更自动失效）。
- CI 为 POSIX 环境，发行冒烟随构建自动执行；fnpack 下载在 win32-x64 / linux-x64 / darwin 已钉 sha256（官方未发布 linux-arm64 文件）。

## 实现载体与语言政策

- 构建脚本为 **TypeScript**（`scripts/build-fnos-fpk.ts`），由 Node 24 直接执行（先例 `scripts/stage-cua-helper.mts`）；不新增 `.mjs`/`.js` 白名单条目。
- fnOS 生命周期脚本必须是 shell（平台契约：`cmd/main` 的 start/stop/status、各 init/callback 由 fnOS 直接以 shell 执行）。按 `docs/specs/language-policy.md`「新增 `.sh` 需 spec 批准」条款，**本 spec 批准**新增 `scripts/fnos-fpk/cmd/*`（`main`、各回调、`lib.sh`）为 shell 载体；批准理由：fnOS 应用生命周期唯一支持的执行形态是 shell，且这些脚本不在仓库运行时路径上。

## 包结构契约（fpk 内）

```text
<package>/
├── manifest                  # appname=mode、platform=x86、service_port=3030、desktop_uidir=ui
├── ICON.PNG / ICON_256.PNG   # 64 / 256，由 packages/desktop/build/icon.png 离线生成后入库
├── config/privilege          # {"defaults":{"run-as":"package"}}：专用应用用户运行
├── config/resource           # {}：不声明共享目录/系统集成
├── cmd/                      # 生命周期脚本（main / install / upgrade / uninstall / config 各 init+callback + lib.sh）
└── app/                      # 安装后解包为 $TRIM_APPDEST（target 卷）
    ├── ui/config             # 飞牛桌面入口（统一网关：iframe + /app/mode/ → app.sock）
    ├── ui/images/icon_{64,256}.png
    ├── proxy.mjs             # 网关接入代理（监听 app.sock：前缀剥离 + 令牌注入）
    ├── bin/mode.mjs          # 发行包入口（runner）
    ├── web/ server/ agent/ node_modules/   # build:mode 发行内容原样
    └── runtime/                            # 捆绑的 linux-x64 Node 运行时
        ├── bin/node
        └── LICENSE                         # 再分发 Node 必须随附的许可文本
```

## 运行时契约（状态所有者）

| 状态 | 所有者（唯一写入方） | 持久位置 |
| --- | --- | --- |
| 服务进程（PID） | `cmd/main` | `$TRIM_PKGVAR/app.pid` |
| 网关代理进程（PID） | `cmd/main` | `$TRIM_PKGVAR/proxy.pid`（socket：`$TRIM_APPDEST/app.sock`） |
| 访问令牌 | `cmd/lib.sh`（`mode_ensure_token`） | `$TRIM_PKGETC/token`（0600） |
| 凭据加密密钥 | `cmd/lib.sh`（`mode_ensure_credential_secret`） | `$TRIM_PKGETC/credential-secret`（0600） |
| Mode 数据根 | Mode 自身初始化器（`initializeDataRootNonInteractive`） | `$TRIM_PKGVAR/.mode`（`MODE_DATA_BASE_DIR=$TRIM_PKGVAR`） |
| 工作区 | `cmd/main` 每次 start 解析 | `etc/workspace` > `TRIM_DATA_ACCESSIBLE_PATHS` 首项 > `$TRIM_PKGVAR/workspace` |

- 监听 `0.0.0.0:3030`；端口来源 `manifest.service_port` → `TRIM_SERVICE_PORT`（脚本兜底 3030）。非回环绑定必须带令牌（runner 的既有约束）。
- 鉴权边界（与 `packages/server` 实现一致）：仅 `/api/*` 与 `/ws` 校验令牌；静态资源不拦。首次带 `?token=` 访问即落 HttpOnly Cookie，后续页面内 RPC 复用。
- 桌面入口走 **fnOS 统一网关**（schema 对齐官方 fndepot 应用）：`app/ui/config` 声明 `type=iframe + gatewayPrefix=/app/mode + gatewaySocket=app.sock`；fnOS 校验登录态后把 `/app/mode/*` 转发到 `$TRIM_APPDEST/app.sock`。同源 iframe，异地经中转域/隧道访问也可用，不再依赖 NAS 内网 IP。
- web 构建以 `MODE_WEB_BASE=/app/mode/` 为 vite base：资产、`/api`、`/ws` 全部跟随网关子路径（默认 `/` 时行为不变）。
- `proxy.mjs` 是 app.sock 的唯一监听者：前缀剥离（兼容网关带前缀或已剥离两种转发形态）+ 令牌注入（网关流量已由 fnOS 鉴权，代理补访问令牌 Cookie）；LAN 直连 `http://<nas>:3030` 仍由服务端严格校验令牌，地址见 `etc/access-url.txt`。
- 环境注入：`MODE_DATA_BASE_DIR`、`MODE_CREDENTIAL_SECRET`、`HOME=$TRIM_PKGHOME`、`PATH` 前置 `runtime/bin`（agent 子进程内 `node` 即捆绑运行时）。

## 事件顺序

```text
安装：fnOS 解包（app→target）→ cmd/install_callback
        ├─ 生成 token / credential-secret → etc/（0600）
        ├─ chown target、etc、var；chmod 755 runtime/bin/node、bin/mode.mjs
        ├─ 写 etc/access-url.txt（含令牌；ui 入口为静态网关配置，无需改写）
        └─ TRIM_TEMP_LOGFILE 输出访问地址（含令牌）
启动：AppCenter → cmd/main start
        ├─ re-exec 到 bash（防 sh/dash 调用）；Node --version 预检（失败时回退 $TRIM_PKGVAR 副本）
        ├─ runner(mode --web) ──PID→ var/app.pid → server(entry-http) → 按会话 spawn agent（同一捆绑 Node）
        ├─ proxy.mjs ──PID→ var/proxy.pid，监听 target/app.sock（网关请求 → 127.0.0.1:3030）
        └─ 失败：日志尾部写入 TRIM_TEMP_LOGFILE（应用中心展示）；代理失败仅 WARN（LAN 直连可用）
访问：飞牛桌面入口（同源 iframe /app/mode/，登录态由 fnOS 校验）→ 代理注入令牌 → /api、/ws
      LAN 直连：http://<nas>:3030/?token=…（etc/access-url.txt）
停止：AppCenter → cmd/main stop → SIGTERM → 10s → SIGKILL 升级
升级：AppCenter 停服务 → 替换 target → cmd/upgrade_callback（重写令牌/权限）→ start
```

## Windows 打包的执行位问题

fnpack 在 Windows 上打包不写入执行位（tar 成员一律 0666，官方工具行为）。处置：
- 构建脚本对最终 fpk 重打包，用 GNU tar `--mode=a+rx` 强制 `cmd/*` 可执行，并对结果断言；断言失败即失败（不产出可疑产物）。
- 捆绑 Node 与入口脚本的执行位在目标机由 `install_callback`（chmod 755）补齐；`cmd/main` 启动前再防御性 chmod；若 target 卷属主不允许，回退为复制到 `$TRIM_PKGVAR/runtime/bin/node` 执行（属主即运行用户）。

## 验收（构建脚本自动执行）

1. fnpack 校验/打包成功；最终 fpk 断言：`cmd/*` 执行位、`app.tgz` 与 `manifest` 存在、`manifest` 版本与端口正确。
2. payload 断言：`runtime/bin/node` 为 ELF 且 >50MB、`runtime/LICENSE` 存在；`proxy.mjs` 存在；`app/ui/config` 为合法 JSON（入口 `type=iframe`、`gatewayPrefix=/app/mode`、`gatewaySocket=app.sock`）；图标 64/256。
3. 令牌行为与网关链路（对 fpk 内 payload 实测）：无令牌 `/api/server-info` → 401；`/?token=` → 200 且下 Cookie；带 Cookie 的 `/api` 与 `/ws` 可用；`proxy.mjs`（TCP 回退）下带前缀/不带前缀的无令牌请求均 200（令牌注入）、网关入口页 200、经网关 WS 可建立；POSIX 上 SIGTERM 优雅退出（Windows 退出码非 0 属平台语义，脚本接受）。
4. 发行包冒烟：`node scripts/mode-distribution-smoke.mjs <mode-<version>.tar.gz>`（TUI 渲染与键盘退出、Web HTML、server-info、WebSocket、优雅退出）。Windows 默认跳过（TUI/conpty 需交互控制台，`--smoke` 可强制），发布级冒烟在 Linux/macOS 打包机执行；Windows 上由验收 3 的令牌行为测试覆盖 Web 链路。
5. CI 集成（`release.yml` 的 `fnos` job，ubuntu-latest）：随发版自动构建 fpk、附到 Release，并把 `<fpk>.sha256` 片段合并进 `sha256.txt`（合并清单对 `.fpk` 有覆盖断言）；全部构建侧验收（含发行冒烟）在 CI 上自动执行。
6. 真机（待用户）：安装 → 桌面入口打开 → 会话可用 → 停止/启动/升级/卸载。

## 已知边界 / 非目标

- 统一网关已接入（见运行时契约）：网关只负责登录态校验、不校验业务权限；代理不解析也不信任 `X-Trim-*` 头，只信任 app.sock 这条受 fnOS 管理的通道。
- 不打包 Electron 桌面 GUI、CUA（Windows-only）、远端桌面（win32）。
- 浏览器自动化插件不预置浏览器（需用户自行安装，NAS 上属可选项）。
- 无多用户/权限体系：令牌即唯一凭证；不信任 `X-Trim-*` 头。
- 入口 iframe 与飞牛桌面同源（同一网关域），令牌 Cookie 为第一方；桌面入口不携带令牌（由代理注入）。
- `etc/access-url.txt` 的直连地址只在浏览器能直达 NAS 时可用（内网 IP 或 Tailscale 等）；异地请用桌面入口（走中转域）。
- 设置页「局域网访问」的被连能力由服务端 lanAccessHost 提供（见 [lan-host-server-mode.md](./lan-host-server-mode.md)）：NAS 可作为被连端供桌面端配对挂接。
- arm64 未验证（发行侧有 linux-arm64 目标，fnOS ARM 版尚不普及）。
- fnOS 手动安装的版本比较对带点预发布后缀（如 `0.0.21-dev.2`）不友好：真机实测会被判为「已安装相同或更高版本」而拒绝；fpk 发版使用纯数字版本（`0.0.21`），避免点分预发布后缀。
- 自动更新跟随 fpk 升级，不接 Mode release catalog。
- Electron 桌面端功能（窗口、auto-update、原生对话框）不在 Web 形态内，属预期降级。
