# 电脑控制（Computer Use）Windows 一期：自研 runtime 与 Helper

## 背景

本仓库随附的 `@mode/cua` 是 API 兼容占位包（`packages/mode-cua/README.md`、NOTICE 第一节均已声明）：
`createComputerUseRuntime`、broker RPC、Helper 安装/启动/校验、帧契约全部 fail-closed，调用返回
"Computer Use is not available in this build."。上游 `zai-org/ZCode` 公开仓库（Apache-2.0）的
`packages/zcode-cua` 发布的同样是占位包——真实现从未开源，只存在于官方产品的编译产物中。

2026-10-09 拍板的四项前提（本 spec 按此展开）：

1. **开源自研**（路径 B）：不搬运官方安装件产物，不读官方实现代码。
2. **纯净室**：规范源只有三类——官方插件市场分发的 `docs/computer-use.md` + SKILL + SDK
   （`computer-use-client.mjs`）、本仓 UI/host 侧消费方代码、Windows 公开 API。
3. **Windows 先行，mac 后补**：一期只交付 win32；mac 分支（`createProductCuaHelperHost`、TCC 面板）
   保持现状 fail-closed，二期再实现。
4. **一期覆盖全部 14 个工具**；能力层选型 **Rust 原生 addon**（napi-rs + windows-rs，对齐官方
   `ax_native.node` 的形态）。

## 范围与非目标

**做**：

- `@mode/cua` 真实现：runtime（14 工具分发、会话/`state_id`/帧）、broker 服务端与客户端、
  frame-contract 六函数、`createCuaProductMcpServerResolver` 等 resolver 关键面。
- Helper 子进程入口（TS bundle）：fork IPC 握手、named pipe broker 服务、addon 加载。
- Rust addon（新 crate）：UIA3 树读取、窗口枚举与启动、截图、输入注入。
- 构建发布：Rust 工具链接入、`dist-cua-helper` 构建产物、`runtime-manifest.json` 生成、
  electron-builder `resources/tools/cua-helper` 打包。
- 测试与实机验收。

**不做**：

- 插件/SDK/UI 改动（官方 marketplace 插件 0.6.3 与本仓全部 UI 消费方零改动）。
- macOS/Linux 实现；官方 Helper CDN 下载链路（`__MODE_CUA_HELPER_BUILD_ID__` 维持无人消费，不接）。
- 第三方 computer-use MCP 接入。
- a11y 精细策略的深度优化（`strategy:"auto"` 一期以 event 路径为主，UIA pattern 能力按下面工具面
  的兜底规则渐进补齐）。

## 架构与所有权

```
┌─ Renderer (packages/ui，零改动) ──────────────────────────────────┐
│ 输入框入口 useCuaComposerEntry · 卡片 cua.tsx/display · Windows 操作指示器 │
└────────┬──────────────────────────────▲──────────────────────────┘
         │ 插件开关/设置                   │ display + session events
┌────────▼──────── Desktop Main ────────┴──────────────────────────┐
│ windowsCuaOperationIndicator（已有，win32 注入）· 设置页            │
└────────┬─────────────────────────────────────────────────────────┘
┌────────▼──────── packages/services（Helper 生命周期唯一所有者）─────┐
│ node.ts 工厂: win32 → createWindowsCuaHelperHost（已有，真实现）     │
│   ├ resolveWindowsCuaRuntime: manifest+sha256 校验（已有）          │
│   ├ WindowsCuaHelperHost: fork / pipe 健康探测 / 代际恢复（已有）    │
│   └ createCuaProductMcpServerResolver（★stub→本期实现）             │
│      插件启用才启动 host → 捕获 socket/token/marker → 注入 MCP env   │
└────────┬─────────────────────────────────────────────────────────┘
         │ fork(entry, ELECTRON_RUN_AS_NODE=1)
┌────────▼──────── Helper 子进程（★新增：入口 TS bundle）─────────────┐
│ fork IPC: transport_ready/ready/error（线格式对齐已有 parseReadyMessage）│
│ named pipe broker 服务端: line JSON {id,method,params}→{ok,result|error}│
│   └ native addon（★新增：cua_ax.node，Rust + napi-rs + windows-rs）  │
│        UIA3 树 · 窗口枚举/启动 · GDI/PrintWindow 截图 · SendInput     │
└────────▲─────────────────────────────────────────────────────────┘
         │ callBrokerMethod（无口令：socket 路径即凭据，token 仅 Worker 桥）
┌────────┴──────── node_repl MCP host ─────────────────────────────┐
│ server.ts captureComputerUseRuntimeFromEnvironment（已有）          │
│ @mode/cua createComputerUseRuntime（★stub→本期实现）:               │
│   14 工具分发 · 会话/state_id/帧差分 · 错误映射 · CUA_NOT_READY       │
│ cua-broker.ts Worker 进程内桥（已有）· result.ts 帧配对（依赖帧契约）  │
└────────▲─────────────────────────────────────────────────────────┘
         │ mcp__node_repl__js（官方插件 SDK：bridge.call(14 方法名)，零改动）
┌────────┴──────── 模型 ───────────────────────────────────────────┐
│ setupComputerUseRuntime bootstrap · skill/docs 契约 · ComputerUseError │
└──────────────────────────────────────────────────────────────────┘
```

**状态所有者与事件顺序**（一次 `left_click`）：

```
模型 ──mcp__node_repl__js(cell 含 setupComputerUseRuntime)──▶ node_repl host
  host ──bridge.call("left_click", args)──▶ 进程内 cua-broker ──▶ runtime.execute
  runtime: 会话校验(state_id/索引→元素解析/帧绑定) ──callBrokerMethod──▶ helper pipe
  helper ──addon(UIA/输入)──▶ 结果(含 dispatched 判定)
  runtime: 更新收据/action_outcome、签发帧(image@0+ref@1+_meta) ──▶ host
  host result.ts: keepLatestCuaFrame 去重 + findOfficialCuaFrameContentPair 校验
  core result-display.ts: 生成 kind:"cua" display → 会话事件 → 卡片渲染
  （指示器：由 session/tool 事件派生，唯一条件是 cell 代码含 setupComputerUseRuntime——已有逻辑）
```

**所有权表**（唯一写入路径）：

| 状态/资源 | 唯一所有者 | 说明 |
| --- | --- | --- |
| helper 生命周期（fork/健康/代际恢复/孤儿回收） | `services/cua-permission-broker`（已有） | 本期不改 |
| 凭据（socket/authority/refresh marker） | `services` host：socket 路径铸造与捕获（无 token；`pluginAuthority` 仅为 provenance），token 校验仅存在于 node_repl Worker 桥 | resolver 实现只负责「何时启动/捕获」 |
| 会话 `state_id`、diff 基线、帧账本（最近可动作栅格）、app 绑定 | `@mode/cua` runtime（node_repl host 进程内） | 按 `context.session_id + workspace_key` 键 |
| AX 快照、窗口句柄、输入与截图原语 | helper 子进程（Rust addon） | 无会话语义，仅窗口句柄缓存 |
| controller lease（单活跃控制者） | runtime + helper 双侧校验 | helper 侧兜底跨 host 场景 |
| 图帧权威（integrity/digest） | runtime 签发，host `result.ts`/`image-normalization` 校验 | 见帧契约 |

**懒启动与恢复事件序**：

```
首次工具调用 → ensureBrokerAvailable(): host.start()（fork+health）
  ├ 未就绪期间 → runtime 返回非 error 文本块
  │   {kind:"CUA_NOT_READY", reasonCode:"broker_not_accepting", retryable:true, message}
  │   SDK 自动退避重试（≤6 次，250→1500ms）
  └ helper 意外退出 → onUnexpectedExit → 代际恢复（已有）
      恢复期间同上 CUA_NOT_READY；恢复后凭据沿用（已有 lastAgentVisibleTransport 语义）
```

## 接口

### runtime 面（`index.d.ts` 既有签名）

`createComputerUseRuntime({brokerSocketPath, refreshMarkerPath, ensureBrokerAvailable, env})`
→ `execute({toolName, arguments, context, signal})` 返回 MCP 结果；`closeSession`；`dispose`。

- `toolName` = bridge 下发的 14 个方法名之一：
  `list_apps, list_windows, get_app_state, left_click, left_click_drag, scroll, type,
  set_value, select_text, key, paste, perform_action, request_access, stop_computer_control`。
- **成功**：`{content, structuredContent?, _meta?, isError:false}`。
  - 单文本块可 JSON 解析的工具（`list_apps`/`list_windows`/`request_access`）→ 文本块即载荷
    （`list_apps` 是**裸数组**；`request_access` 文本块须是扁平 `AccessStatus`
    `{ready, accessibility:"granted", screenRecording:"granted", message?}`，SDK 直接返回它）。
  - `get_app_state` 无截图 → 单文本块为渲染树；`structuredContent` 承载
    `state_id / base_state_id / snapshot_mode / app / window / focused_element / elements / changes`。
  - 带截图 → `content = [image@0, ref文本@1]` + `structuredContent`（见帧契约）。
- **失败**：`isError:true`，文本块含 `message`，且 `code` 只能取下表 17 键之一
  （SDK `ERROR_CODE_BY_BROKER` 固定映射，未知键一律归 `INTERNAL`）：
  `permission_denied, not_authorized, launch_failed, invalid_request, element_unavailable,
  not_settable, not_selectable, action_unavailable, foreground_required, controller_busy,
  broker_unavailable, version_mismatch, stale_socket, timeout, unimplemented, method_not_found, internal`。
  - 元素消失/索引失效 → `element_unavailable`（SDK→`ELEMENT_UNAVAILABLE`，reobserve）。
  - **stop 之后的变更类调用** → `controller_busy`，`details.owner` 与 message 明示
    "computer control was stopped"（决策：固定 17 码里没有 `control_stopped`，
    `controller_busy` 是 never-retry，语义最接近且不会诱导盲重试）。
- **收据**（SDK `receiptOf` 从顶层 / `structuredContent` / 文本 JSON 合并读取，
  含 `action_outcome` 嵌套）：`state_id, frame_id, action_sent, dispatch_status,
  state_sync_status, code, reason, snapshot_mode, base_state_id`。
  - 变更类动作（`MUTATING_METHODS` 十个）：成功 → `action_sent:true`；
    addon 证明未下发的失败 → `action_sent:false, dispatch_status:"not_sent"`；
    无法证明的失败 → `action_sent:true, dispatch_status:"possibly_sent"`（SDK 会强制 reobserve）。
- **CUA_NOT_READY**：helper 未就绪时唯一合法的「失败」形态（非 error），见上文懒启动序。

### 帧契约（`frame-contract.js` 六函数，消费方已钉死）

- **帧对**：`content[0]` image 块 + `content[1]` 文本 ref 块；`content` 中至多一对（一栅格规则）。
- **ref 文本**＝整块单行 JSON（`isOfficialCuaImageRefText` 只认「整块即此 JSON」）：

  ```json
  {"image_ref":{"frame_id":"<uuid>","credential":"<32hex>","raster_sha256":"<hex>",
   "width":W,"height":H,"mimeType":"image/png"}}
  ```

  `image_ref.frame_id` 是 SDK `frameIdOf` 的读取位（客户端注释明确官方把它放 ref 块内）；
  `credential` 每帧随机，供 `containsOfficialCuaImageRefCredentialText` 配对与孤儿引用 fail-closed。
- **权威 meta**：`_meta["mode.cua/official-frame-integrity-v1"] = {v:1, frame_id, raster_sha256}`
  （真值即可，`hasOfficialCuaFrameAuthority` 只查存在性）。缺 meta 不进权威路径，会被通用归一剥掉。
- **六函数语义**：
  - `isOfficialCuaImageRefText(text)`：trim 后 JSON 对象、`image_ref` 含非空 `frame_id`+`credential`。
  - `containsOfficialCuaImageRefCredentialText(text)`：同上解析且 `credential` 非空。
  - `parseOfficialCuaImageRef(text)`：解析返回上述结构；非法 → `undefined`。
  - `readRasterEnvelopeIdentity(image块)`：`sha256(base64 解码字节)` hex。
  - `findOfficialCuaFrameContentPair(content)`：相邻对 `(i, i+1)` 校验，返回
    `{image, imageRef, imageIndex:0, imageRefIndex:1}`（producer 保证 0/1；投影层强校验）。
  - `preserveOfficialCuaFrameResult(result, {imageProcessorPort, signal})`：**精确栅格闸门**——
    `ref.raster_sha256` 与实际 digest 不符 → 整帧 fail-closed（移除 image+ref，替换为与
    `official-cua-media` 同语义的不可见提示文本；注意 `mode-cua` 不得反向依赖 `core`，
    文案在本包内自持、字面与 core 侧对齐）；base64 > 200KiB → 经 `imageProcessorPort`
    压到限内，压不进 → 同样降级为不可见提示；通过则原样保留帧与 meta。
  - `attestOfficialCuaFrameContent(content, "official_cua_frame_v1")`：终检 image@0+ref@1+
    digest 匹配 → 返回真值；否则 `call-runner` 抛 "final model-content attestation" 失败。
  - `containsImageRefAuthority(text)`：ref 合法且含 credential/frame_id（供序列化上下文过滤）。
- **尺寸策略**：helper 侧截图后按质量阶梯重编码，保证 base64 ≤ 200KiB（质量保底仍超 →
  返回 `internal` 错误 + message，**不发超限帧**——超限帧会被归一化层换成 artifact 文本，
  模型将失去坐标基准）；owner/层 = **helper-backend capture 出口**（PNG→JPEG 质量阶梯由
  Plan B 首个任务实现——帧只在 Plan B 流动，一期不实现）。

### 工具面（14 个；参数与语义以插件 `docs/computer-use.md` 为规范源）

参数严格按 docs 的签名（SDK 已做 `unrecognized_keys` 校验，runtime 仍需二次校验）。要点：

- `app_ref`：`{name|bundle_id|pid, window_id?}`；裸字符串按 `bundle_id` 解析。
  Windows 语义：`name` = OS 列出的显示名（Start 菜单名），**不是窗口标题**；
  `bundle_id` 优先 AUMID，回退可执行文件路径——同一值用于 `targetApp.iconLocators`
  （`windows-aumid` / `windows-executable-path`，schema 见 `toolDisplay.ts`）。
- `target`：`{type:"element",index}` 或 `{type:"coordinate",x,y,frame_id?}`。
  索引身份 = app + index（wire 上无观察凭证）；索引须来自该 app 最新一次观察，未观察就动作
  → `STALE_STATE`（SDK 本地产生）；观察后元素消失 → `element_unavailable`。
  坐标省略 `frame_id` 时绑定会话最近可动作栅格；无栅格 → `invalid_request`
  （message 复述 "no actionable frame is available in this transport" 语义）；
  栅格过期/owner 不符 → `invalid_request`（`frame_dispatch_identity_mismatch` 语义）。
- `get_app_state`：`include_screenshot` 帧对随行；`disable_diffing` 全量树。
  diff 基线 = 该会话已展示给模型的最近树；首观察与截图后首观察必为全量。
  文本树格式（UI `cuaResultState` 逐字解析）：
  - 首行 `app: <name> pid=<N> "<title>"`——`name` 须 sanitize 到 `[A-Za-z0-9.-]+`
    （结构化 `app.name` 可保留原始显示名，两处允许不同）。
  - 元素行 `[<idx>] …`，含 `name = value` 形式与该元素 `actions` 列表
    （UI 提取 target chip：`/^\s*\[\d+\]\s+(.+)$/` + `name = value` 启发式）。
  - 提示行：diff 头、`indices are sparse`、`showing A-B of N items`、
    `[effect_evidence unchanged]` 按 docs 实现。
- `strategy:"event"` 要求目标 app（与 `window_id`）已前台，否则 `foreground_required`
  且**不下发任何输入**；`"auto"` 先 a11y 后 event。
- `set_value`：优先 UIA ValuePattern（后台可用）；不支持 → 聚焦 + 全选 + `type` 的 event 兜底；
  均不可 → `not_settable`。
- `select_text`：UIA TextPattern `Select(range)`；元素不支持 → `not_selectable`
  （歧义匹配拒绝而非取第一个，按 docs）。
- `perform_action`：只接受该元素 `actions` 列出的动作，未列出 → `action_unavailable`。
- `paste`：写系统剪贴板 → 粘贴 → 恢复用户剪贴板；无 app 消费 → `timeout`
  （owner = **Plan B 运行时**：settle 后依观察结果判定；一期 addon/backend 返回 `dispatched` 即可）；
  后台 app → `foreground_required`（docs 明示 paste 走 event）。
- `key`：xdotool 风格 keysym + 短名（`Return/Tab/Control_L+a/super+c/Up`…，
  Windows 用 `ctrl`），`repeat`/`hold_seconds` 支持。
- `scroll`：`scroll_amount` 页数 clamp 0–100。
- `list_windows` 行：`index, window_id, title(""|null 语义按 docs), bounds, main, focused,
  onscreen`；`subrole`/`text_preview` 为 mac 字段，Windows 不产出（docs 标注 macOS only）。
- `request_access`（Windows 无 TCC）：文本块 = 扁平 `AccessStatus`（全 granted/ready）；
  `structuredContent` = `{platform:"windows", backend:"uia",
  accessibility:{status_after:"not_required"}, screen_recording:{status_after:"not_required"}}`
  供 `cuaAccessDetails` 渲染。**不设** `_meta[CUA_REQUEST_ACCESS_STATUS_META_KEY]`——
  `cuaRequestAccessStatusSchema` 是 `platform:"darwin"` strict schema，Windows 载荷
  安全解析失败后 display 自然省略该字段。
- `stop_computer_control`：释放 lease、置 stopped（见错误码决策）、后续变更类动作
  `controller_busy`；幂等可重复调用成功。
- 观察等待：runtime 在树/截图采集前做有界等待（docs："Observations wait an appropriate
  amount of time before capturing"），由 helper 侧 settle 逻辑实现，不让模型 setTimeout。

### broker 协议（helper pipe，线格式自定、双端同源）

- 行分隔 JSON：请求 `{id, method, params}`，响应 `{ok:true, result}` 或
  `{ok:false, error:{code, message, details?}}`（`broker.d.ts` 的
  `parseRequestLine/dispatchRequest/errorResponse*` 契约）。
- **method 集**（runtime→helper 的原语层，与 14 工具名解耦）：
  `health, list_apps, list_windows, observe, capture, perform, launch_app, screen_probe`。
  `isBrokerMethod` = 白名单前缀；`isReadOnlyBrokerMethod` 覆盖 6 个只读方法
  `health, list_apps, list_windows, observe, capture, screen_probe`（与 `broker.js` 的
  `READONLY_METHODS` 逐字一致）。
- **鉴权**：helper 连接**无口令**，socket 路径即凭据（`packages/shared/src/runtimeEnv.ts:149`
  注释拍板：socket 不可猜即可；token 概念仅存在于 node_repl 进程内 Worker 桥，非 helper 连接）；
  socket 由 services host 铸造并经既有 env 注入链路传递，日志不落凭据。
- **fork IPC 握手**（`parseReadyMessage` 既有线格式，实现必须对齐）：
  `{protocol:"mode-cua-windows-dev/v1", type:"transport_ready"|"ready"|"error",
    socketPath, pid, ...}`；`transport_ready` = 管道已绑定鉴权，`ready` = 完整健康握手。
- **addon 版本握手**：helper `health` 返回 addon 协议版本，与 runtime 常量不一致 →
  `version_mismatch`（SDK→`VERSION_MISMATCH`，never-retry）。
- **并发与线程**：UIA pattern 调用走 STA 串行队列（UIA STA 线程亲和）；SendInput/剪贴板注入在调用方线程执行（不触目标消息泵）；GDI 截图与 observe 同队列串行（非并行）；单请求超时 → `timeout`。
- 错误码在 helper 侧就用 17 码表生成；`unimplemented`/`method_not_found` 保留给分发层。

### controller lease

- helper 单实例（由 services 代际管理保证），lease 按 `(session_id, workspace_key)` 抢占；
  异 workspace 并发 → `controller_busy` + `details.owner`（SDK never-retry，要求用户处理）。
- `closeSession` 释放本会话 lease 与帧账本；`dispose` 关闭连接并清理。

## 构建与发布

- **Rust workspace**：`crates/mode-cua-ax/`（napi-rs），features 覆盖
  `Win32_UI_Accessibility / Win32_UI_Input_KeyboardAndMouse / Win32_Graphics_Gdi /
  Win32_UI_WindowsAndMessaging`。产出 `cua_ax.node`（N-API 稳定 ABI，Electron 与
  `ELECTRON_RUN_AS_NODE` 的 Node 通用）。
- **构建脚本** `scripts/build-cua-helper.mjs`：
  `cargo build --release --target x86_64-pc-windows-msvc` → esbuild 打 helper 入口 →
  汇出 `dist-cua-helper/{entry.cjs, cua_ax.node, runtime-manifest.json}`；
  manifest：`schemaVersion:1, packageName:"@mode/cua", packageVersion, platform:"win32",
  arch, electronVersion(取桌面端 electron 版本，CI 注入), entry, addon, sha256{entry, addon}`
  ——与 `resolveWindowsCuaRuntime` 既有校验逐字段一致。
- **dev 根契约**：`packages/mode-cua/package.json` 增加
  `modeCuaRuntime:{schema:1, windows:{entry, nativeAddon}}`（相对包根的受包含路径，指向
  构建输出，gitignore）；`MODE_CUA_DEV_ROOT` 指到 `packages/mode-cua` 即可走
  `requireDevelopmentRuntimeRoot` 既有解析。
- **打包**：electron-builder `extraResources` 仿 ripgrep 模式增加
  `{from:"bundled-tools/<platform>/cua-helper", to:"tools/cua-helper"}`，仅 win32 目标 stage
  （prepare 步骤把 `dist-cua-helper` 复制进 `bundled-tools/win32-x64/cua-helper`）。
  打包态由 `resolvePackagedRuntime`（`resources/tools/cua-helper`）既有逻辑解析。
- **CI**：windows job 装 rustup + cargo 缓存；`cargo test` 与构建纳入 verify；
  typecheck/lint/architecture:check 口径不变。release 产物文件名不变（helper 内嵌）。
- **明确不接**官方 Helper CDN 下载（`MODE_CUA_HELPER_BUILD_ID` 维持无人消费）。

## 测试与验收

1. **TS 单测**
   - 帧契约六函数全分支：伪造 ref（非整块 JSON）被剥、孤儿 ref fail-closed、digest 不符整帧
     拒绝、>200KiB 压缩/降级、attest 终检。
   - 收据与错误映射：17 码表、`possibly_sent` 三态、CUA_NOT_READY 冷启动（含 retryable=false 分支）。
   - 状态机：diff 基线轮换、索引重编号、稀疏/截断提示、窗口切换（模态成为捕获窗口）。
   - resolver：插件开关注入 MCP 凭据、关不启动 host；mac 分支保持 fail-closed。
   - display 黄金样例：runtime 输出 → `createCuaToolResultDisplay` → UI 解析（首行 header、
     元素行、截图 dataUrl、errorCode）全链路可解析。
2. **Rust 单测**：树行走（fixture Win32 窗口）、keysym 映射、region clamp、digest/编码阶梯、
   错误码映射。
3. **集成（win32，`CUA_INTEGRATION=1` 门）**：dev 根起 helper → IPC 握手 → token 鉴权 →
   对记事本 fixture 完成 observe→click→type→re-observe 往返 → CUA_NOT_READY→重试成功 →
   kill 子进程 → 代际恢复。
4. **E2E**：现有 desktop e2e harness——开启插件后入口出现；会话内 SDK cell `list_apps` 成功；
   指示器点亮条件（cell 含 `setupComputerUseRuntime`）。CI GUI 能力受限时如实标注，
   由实机清单兜底。
5. **实机验收清单**：记事本输入往返；设置页（Electron/Chromium 树）观察与点击；截图卡片渲染；
   元素消失 → reobserve 语义演示；`stop_computer_control` 后变更动作被拒；插件关闭 →
   一键回到明确的不可用态；`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。

## 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| UIA 读取 Chromium/Electron 树质量不足 | Week-0 探针（≤2 天）：先对设置页窗口出树；不足则补 MSAA fallback 或缩小首期承诺 |
| UIPI：向提升（管理员）窗口注入被拒 | 注入前完整性预检（`GetWindowThreadProcessId` → `OpenProcessToken` → `GetTokenInformation(TokenIntegrityLevel)` 与本进程 RID 对比）：目标严格更高 → `action_unavailable` + 指引（同等权限重启目标/退出提升；不做提权重构）；探测任一步失败 → 放行（fail-open：设施故障不阻断合法链路）；event 与 UIA 两条路径都在任何注入之前执行 |
| 防截屏/受保护内容黑帧 | 截图后做非空校验，失败 → `internal`/`timeout` + 明确 message |
| 目标窗口挂起时 PrintWindow 占死共享 STA 队列 | IsHungAppWindow 预检 → 直接 BitBlt 回退；调用中途挂起的残余风险保留为已知限制 |
| 锁屏期间采集失败 | 统一 `permission_denied` + message（17 码表无 screen_locked，选语义最近且 never-retry）；owner = **Plan B**（Plan B 起在 helper-backend capture 前接 `screen_probe` 预检，锁屏 → `permission_denied`；一期未接线属已知归属注记） |
| 纯净室对官方语义的偏差 | 以市场分发的 docs+SDK 为规范源，集成测试锁定行为；偏差只允许更保守 |
| CI 时长/工具链 | Rust 仅 windows job；cargo 缓存；不引入跨平台矩阵 |
| 新增 Rust 维护面 | 单 crate、接口粗粒度（8 个原语），TS 侧不暴露 addon 细节 |

## 开放点（实现期自行钉死，不阻塞设计）

- `callBrokerMethod` 是否走 env 内 token 的精确字段（读 `mcp-config`/`broker.js` 现有真实现对齐）。
- helper 入口 `ready` 消息的完整字段集（`parseReadyMessage` 已定主干，补齐细节）。
- `MODE_CUA_HELPER_ADDON` env 与 `loadRealNativeAddon` 的装载顺序（按 `broker-server.d.ts` 语义）。
- 截图格式阶梯（PNG→JPEG 质量序列）与 `zoom/region` 参数的具体默认值（按 docs 截图节微调）。
