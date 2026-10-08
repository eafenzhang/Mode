# 收窄语言 + Rust/Tauri 重写 + 产品自研：分阶段总体规划

> **For agentic workers:** 本文件是项目级路线图，不是单次可执行计划。每个阶段（M0–M5）在开工前必须按 `writing-plans` 技能单独产出一份细粒度子计划（含逐步骤、测试与提交点），存放到 `docs/plans/YYYY-MM-DD-<阶段名>.md`，本路线图只锁定阶段边界、验收口径与决策记录。

**Goal:** 把产品收敛为 TypeScript（前端/工具链）+ Rust（核心/平台）两门语言，用 Rust + Tauri v2 重写全栈，并在重写过程中以 clean-room 纪律实现与上游 `zai-org/ZCode` 的版权切分，达成真正的自研。

**Architecture:** 同仓库绞杀者演进——现有 Electron/TS 栈冻结为"参照实现 + 测试预言机"，新增 `crates/`（Rust workspace）与 `apps/desktop-tauri/`（Tauri v2 壳）；协议类型双端生成并对拍；插件生态以 Node sidecar 按需下载的方式保留在核心之外；体积目标（初装 ≤30MB 核心）由"无 Chromium、无捆绑 Node 核心 + sidecar 按需"达成。

**Tech Stack:** Rust 1.8x / Tauri v2（tauri-plugin-updater、deep-link、os、tray）、tokio、axum、russh、ratatui、rmcp、portable-pty、notify、schemars/typeshare、React 18 + TS 5（存量前端）、Node sidecar（存量插件宿主）。

**Spec:** 每个阶段的实现 spec 在阶段开工时于 `docs/specs/` 新建；本路线图引用的既有真源：`AGENTS.md`（架构策略）、`DESIGN.md`（UI 规范）、`docs/specs/p2-mode-naming.md`（必须保留的合规段落）、`third-party/copied-components.json`（第三方台账）。

## Global Constraints

- **clean-room 纪律**：新代码只允许从本仓库 spec/协议/测试/公开文档出发编写；任何任务禁止阅读、引用、翻译 `zai-org/ZCode` 及本仓库中上游衍生的实现文件（例外：作为黑盒对拍的**运行行为**，不看实现）。
- **语言政策**：新代码只允许 Rust、TypeScript、HTML、CSS；shell 仅限 4 个既有胶水文件，新增需 spec 批准；`.agents/skills/**` 视为第三方 vendored，不参与语言收窄。
- **不推送**：agent 只做本地提交；push 由人发起（push main 会触发自动发版，带 `[skip release]` 可跳过）。
- **合规不回退**：`LICENSE-APACHE`、`NOTICE.md` §4 衍生声明、`THIRD-PARTY-NOTICES.md` 台账在任何阶段不得削弱；M4 前所有发布物仍属 Apache-2.0 衍生作品，公告口径照实写。
- **三平台**：Windows x64、macOS arm64、Linux x64 全程 CI 出包；体积口径统一按 NSIS/Tauri NSIS 安装包字节数对比。
- **版本与工具**：Node 24.14.0、pnpm 10.33.2（`mise.toml` 为准）；Rust 工具链在 M0 钉版本（`rust-toolchain.toml`）。
- **既有语义不得丢失**：owner/lease、CommandInbox 串行准入、`desktop-continuous` 与 `web-remote-replayable` 双链路语义、workspaceIdentity 贯穿——每阶段对拍清单必须覆盖。

## 三目标的合并逻辑（为什么合成一张路线图）

1. **语言收窄不在旧仓库全面执行**：旧栈 56% 是上游代码、终将被替换，在它上面做 122 个 `.mjs → .ts` 大迁移是消耗性工作。旧仓库只做两件零风险动作（vendored 标记、语言政策冻结），收窄在新代码里天然成立。
2. **自研通过重写实现**：重写即换血。只要全程 clean-room，M4 结束时"与上游逐文件 diff 清零"自然成立，不需要单独的清洗项目。
3. **体积由架构解决**：Tauri 核心无 Chromium；Node 运行时不进初装包，随插件 sidecar 按需下载（复用既有 `MODE_REMOTE_ASSET_CDN_BASE_URL`/`prepare:remote-assets` 基建）。

## 先拍板的五个决策（M0 决策记录，DR-01…DR-05）

| # | 决策 | 结论 | 依据 |
|---|---|---|---|
| DR-01 | 插件宿主 | Rust 核心 + **Node sidecar 子进程**跑存量 TS 插件/MCP 工具/Skills；sidecar 首启按需下载 | 插件生态是 npm/TS（272 插件目录、Claude/Codex 兼容），换引擎=放弃生态；插件属第三方代码，不在"自研"口径内 |
| DR-02 | 前端 UI 策略 | 骨架期（M1–M3）**可挂载现有 React UI** 加速联调（开发期不分发，不产生额外义务）；**M4 完成 clean-room 替换**，1.0 发布前删除全部上游衍生 UI | `packages/ui` 有 15.8 万行仅改名上游代码，终态必须替换；但第一版就重写 UI 会拖死骨架进度 |
| DR-03 | 上游同步 | **立刻停掉** `upstream-audit.yml` 的定时同步（改 `workflow_dispatch`） | 继续同步=持续引入上游新版权，自研永远还不完 |
| DR-04 | 上游基线 | 把上游 main 导入为本仓库 `upstream/zcode-baseline` 分支（一次性 fetch + branch） | 兑现 NOTICE"对照上游仓库 diff 追溯"；给 M4 的 diff 清零验收当标尺 |
| DR-05 | 仓库形态 | **同仓库并存**（`crates/`、`apps/desktop-tauri/` 与旧栈同仓），旧栈打 EOL 后归档 | 复用协议定义、测试基线、CI 与第三方台账；避免双仓漂移 |
| DR-06 | 与电脑控制（CUA）一期的归口 | 电脑控制并行会话已在做 **Rust addon（napi-rs + windows-rs）+ Rust 工具链接入 + electron-builder 打包**（`docs/specs/computer-use-windows-runtime.md`）——**Rust 工具链、cargo CI job、electron-builder 打包规则以先落地的一期为准**，M0 的 C# helper Rust 化跟进其约定；两者同期只允许一处改 `electron-builder.config.js` | 电脑控制一期实质上就是路线图要的"Rust 先行试水"，重复引入两套 Cargo workspace/CI 只会互相打架 |

---

## 总图（2–3 人 ≈ 12–17 个月；单人 ≈ 18–30 个月）

```
M0 地基与纪律   ▸ 2–4 周    语言冻结/合规基线/对拍机制/Rust 先行试水
M1 Tauri 骨架   ▸ 2–3 月    壳+平台服务层+协议双端+三平台出包
M2 运行时 Rust  ▸ 4–6 月    agent 会话/工具/权限/双链路语义
M3 生态层       ▸ 3–5 月    sidecar/插件市场/IM bot/LAN/远程/Web/TUI
M4 自研验收     ▸ 2–3 月    UI clean-room 替换+上游 diff 清零+台账重建
M5 发布收口     ▸ 1–2 月    更新链/签名/旧线 EOL/软著商标
```

---

## M0 地基与纪律（2–4 周）

**目标：** 把"语言收窄、自研纪律、可验收基线"三件事在旧仓库先钉死，并用一个小 Rust 构件验证工具链。

**Files:**
- Modify: `.gitattributes`、`.github/workflows/upstream-audit.yml`、`AGENTS.md`
- Create: `docs/specs/clean-room-policy.md`、`docs/specs/language-policy.md`、`scripts/check-language-policy.mjs`、`rust-toolchain.toml`、`crates/browser-import-helper/`（Rust 版 Windows 浏览器数据导入 helper）
- Modify（协调后）: `packages/mode-cua/**`（JS → TS，归口并行会话）
- 可选（过渡期发版线瘦身）: `packages/desktop/electron-builder.config.js`（修 asar 重复打包 + `compression: max`，见上轮实测：147MB → 目标 ~105MB）

**Interfaces:**
- Consumes: 无（起点）。
- Produces: `scripts/check-language-policy.mjs`（CI 调用，退出码 0/1）、`crates/browser-import-helper` 的 CLI 契约（参数与现有 C# 版逐字节对齐）、clean-room 规则文本（后续所有阶段的 PR 检查项）。

- [ ] **语言统计收窄**：`.gitattributes` 追加 `.agents/skills/** linguist-vendored`（它是 `copied-components.json` 台账在案的第三方复制件，语义成立）。效果：GitHub 语言条 C# 2.2%→0.2%、Python 0.6%→0、HTML 11.5%→0.1%（HTML 的 11.5% 实为 `xlsx/docs/cases/superstore…/superstore_orders_print.html` 单个 4.78MB fixture）。
- [ ] **停上游同步（DR-03）**：`upstream-audit.yml` 删除 `on.schedule`，仅留 `workflow_dispatch`；提交信息注明决策号。
- [ ] **导入上游基线（DR-04）**：`git fetch https://github.com/zai-org/ZCode.git main` → `git branch upstream/zcode-baseline FETCH_HEAD`（本地分支，由人决定何时推送）。
- [ ] **clean-room spec**：新建 `docs/specs/clean-room-policy.md`，写明：禁读上游实现的适用范围（仓库内外所有新代码任务）、AI 提示词模板中的禁用引用、对拍只比行为不看实现、违规处置；`AGENTS.md` 增加一行指针。
- [ ] **语言政策 + 门禁**：`docs/specs/language-policy.md` + `scripts/check-language-policy.mjs`——扫描 `git diff --cached --name-only`，非 vendored 路径新增 `.py/.cs/.js`（`.mjs/.cjs` 亦拒）即退出 1；挂进 `pnpm verify:pre-push` 与 CI。验收：故意暂存一个 `foo.py` 跑脚本 → FAIL；移除 → PASS。
- [ ] **Rust 工具链落地（遵循 DR-06）**：先确认电脑控制一期的 Rust workspace/工具链落点与 cargo CI job 形态（`docs/specs/computer-use-windows-runtime.md`），`rust-toolchain.toml` 与其保持同一版本；`crates/browser-import-helper` 用 Rust 重写现有 1561 行 C#（`packages/desktop/native/windows-browser-import-helper/Program.cs`），**先写行为对拍测试**（同输入 → 与 C# 版输出一致），cargo test 全绿后按一期已建立的打包约定接进 electron-builder，删除 C# 工程。验收：Windows 实机导入一次浏览器数据成功；语言条 C# 归零。
- [ ] **mode-cua JS→TS（让行）**：等电脑控制一期收口后再执行（该目录正在被并行会话开发）；`.d.ts` 手写声明删除，改由 tsc 产出。
- [ ] **（可选）过渡版瘦身**：按上轮结论修 `files` 白名单（echarts/mermaid/pdfjs/lucide/shiki/katex 等渲染层库已在 `out/renderer` bundle 内，asar 不再重复装其 node_modules）+ `compression: "max"`，本地出包对比体积并记录。**与 CUA 一期对 `electron-builder.config.js` 的改动错峰，同一窗口期只动一处。**

**阶段验收：** 语言政策门禁在 CI 生效；`upstream/zcode-baseline` 存在；clean-room spec 合入；Rust helper 替换 C# 后三平台包可出；GitHub 语言条只剩 TS/TSX + JS（旧脚本）+ HTML/CSS。

---

## M1 Tauri 骨架（2–3 个月）

**目标：** Tauri v2 壳跑通存量 React UI 与平台服务层，三平台出包，初装体积出现数量级下降。

**Files:**
- Create: `apps/desktop-tauri/`（`src-tauri/`：taui v2 工程 + `Cargo.toml` workspace 根挂 `crates/*`）、`crates/platform/`（IPlatformService 的 Rust 对应）、`crates/protocol/`（schemars + typeshare）、`crates/updater/`、`crates/tray/`、`crates/deeplink/`
- Modify: `pnpm-workspace.yaml`（纳入 `apps/desktop-tauri`）、`.github/workflows/release.yml`（新增 tauri 出包 job）、`docs/specs/tauri-platform-commands.md`（本阶段 spec，开工时先写）

**Interfaces:**
- Consumes: 存量 `packages/ui`（DR-02 挂载）、`packages/shared/src/platform.ts` 的 `IPlatformService` 方法清单（作为 Rust 命令面的**接口规格**，只读签名与文档，不读实现）。
- Produces: `platform::commands`（`#[tauri::command]`，与 `IPlatformService` 逐方法同名同参）、`protocol` crate 导出的 TS 类型（`typeshare` 生成 `packages/shared/src/generated/tauri-protocol.ts`）、`ModeTauri.exe` 安装包（≤25MB 目标）。

- [ ] **脚手架**：`cargo create-tauri-app`（react-ts 模板）落 `apps/desktop-tauri`，`rust-toolchain.toml` 已在 M0；CI 加 `cargo test` + `cargo clippy -D warnings`。
- [ ] **平台命令面**：按 `IPlatformService` 签名清单逐个实现（文件对话框、剪贴板、通知、托盘、深链、代理设置、更新检查 stub、shell open）；每个命令先写 tauri::test / 集成测试再实现（红→绿）。
- [ ] **协议双端**：新 Rust 结构体 `#[derive(schemars::JsonSchema, Serialize, Deserialize)]` → typeshare 导出 TS；CI 断言生成文件无漂移；存量 TS 协议保持真源，Rust 侧 serde 类型 + 契约测试对拍（复用 `packages/services` 的 contract 测试思路）。
- [ ] **UI 挂载（DR-02）**：现有 renderer 产物以 `frontendDist` 方式挂进 Tauri；验证窗口内 UI 正常、RPC 走 `platform::commands` 适配层（`window.mode` shim → `invoke()`）。
- [ ] **三平台出包**：release.yml 增加 tauri job（`tauri build`），产出 `Mode-<版本>-<平台>-<架构>.<ext>` 命名一致的产物；记录三平台安装包体积基线。
- [ ] **更新链 stub**：`tauri-plugin-updater` 接现有 GitHub Releases 清单格式（与旧线共用 feed，版本号延续）。

**阶段验收：** 三平台安装包全绿且 Windows ≤30MB（争取 25MB；renderer 资产 53MB 解包口径是主要变量）；Tauri 版可完成"打开窗口 → 渲染 UI → 走通 10 个平台命令"的 E2E；`cargo clippy -D warnings` 零告警；不带任何 Node 运行时。

---

## M2 Agent 运行时 Rust 化（4–6 个月）

**目标：** 把会话循环、工具执行、权限引擎、双链路语义搬到 Rust，且与旧 TS 运行时**行为对拍**通过。

**Files:**
- Create: `crates/agent-core/`（session loop、streaming、CommandInbox、owner/lease、权限引擎）、`crates/agent-tools/`（fs/git/shell/pty[portable-pty]/browser 桥）、`crates/agent-protocol/`（stdio/WS 帧，对应 `packages/shared/src/mode-protocol`）、`docs/specs/tauri-agent-runtime.md`
- Test: `crates/agent-core/tests/`（对拍套件）、复用 `pnpm --filter @mode/services test` 作为行为规格

**Interfaces:**
- Consumes: `crates/protocol`（M1）、`crates/platform`（工具要弹审批 UI 走平台命令）。
- Produces: `agent_core::runtime::Runtime::run(prompt) -> Stream<Event>`（事件枚举与 TS 侧 `mode-protocol` 事件一一对应，typeshare 导出）、`agent_tools::registry()` 工具声明表（name/schema/权限等级，供权限引擎与 UI 审批用）。

- [ ] **只读工具先行**：read/glob/grep/list —— 先写对拍测试（同一 prompt 分别喂旧运行时与新运行时，比较工具调用序列与最终文件快照），再实现，全绿提交。
- [ ] **文件写与 diff 呈现**：edit/write/patch 工具，接入审批（allow-once / allow-always / project-always 三态持久化，复用旧版数据格式）。
- [ ] **shell 与 pty**：portable-pty + CommandInbox 串行准入；对拍含超时、取消、退出码语义。
- [ ] **会话流与重放**：`desktop-continuous`（不落重放）与 `web-remote-replayable`（快照+队列重放）两条链路分别验证——各写一套 E2E，改 stream/snapshot/queue 时双跑。
- [ ] **权限引擎**：规则表驱动（allow/deny/ask + 工具级确认），数据文件与旧版同格式（`M4` 迁移零成本）。
- [ ] **对拍矩阵收口**：把 services 198 个测试中与运行时相关的用例逐个转为 Rust 侧等价测试（转不动的记录原因进 spec），覆盖率报告进 CI。

**阶段验收：** 对拍套件全绿（工具序列 + 文件快照 + 事件流三层一致）；双链路 E2E 各自通过；用真实模型跑通 20 个端到端任务（改代码/跑测试/浏览器）无行为回归。

---

## M3 生态层（3–5 个月）

**目标：** 插件 sidecar、市场、IM bot、LAN 远连、SSH/WSL/Docker 远程、Web 端、TUI 全部落到新栈。

**Files:**
- Create: `crates/plugin-host/`（sidecar 生命周期管理：按需下载/启动/健康）、`crates/marketplace/`（`.mode-plugin` 清单与市场目录，兼容读旧格式）、`crates/lan/`（UDP 发现 + 配对码）、`crates/remote/`（russh / 系统 ssh 调用 + workspace attach）、`crates/server/`（axum，web-remote 链路）、`crates/tui/`（ratatui）、`crates/bots/`（IM 桥：HTTP/WS 直连或 sidecar 复用 TS SDK，按通道逐个决策）
- Modify: `apps/mode-cli`（旧 TUI 标记归档）、`docs/specs/` 对应各域 spec（沿用既有 `lan-*`、`remote-workspace-identity`、`plugin-store-availability` 等 spec 作为行为规格）

**Interfaces:**
- Consumes: `agent_core::runtime`（插件工具注入点）、`crates/marketplace`（M2 的工具注册表）。
- Produces: `plugin_host::Sidecar::ensure()`（返回可执行路径，缺失则按 manifest 下载）、`server::app()`（axum Router，路由与旧 server 一一对应，契约测试对拍）。

- [ ] **Node sidecar 打包**：按 DR-01 —— Node 运行时 + 插件宿主 bundle 作为独立资产上 CDN；`plugin_host::ensure()` 首启用时下载、校验 sha256、健康探测；本地开发直连不下载。验收：断网时核心功能全在，仅插件功能提示"需下载组件"。
- [ ] **插件市场迁移**：读旧 `installed_plugins.json`/marketplace 缓存原样可用；市场卡片 E2E 复用现有 `plugin-store-availability` spec 场景。
- [ ] **IM bot 逐通道**：飞书/企业微信/钉钉走 HTTP+WS 直连（reqwest/tungstenite），有 TS SDK 深依赖的通道先走 sidecar；心跳、群/私聊模式、会话绑定沿用既有 spec 与数据文件格式。
- [ ] **LAN 远连**：UDP 广播发现 + 6 位配对码 + 桌面即服务端——对照 `lan-discovery/lan-remote-attach/lan-paired-devices` 三个 spec 逐条验收（含旧客户端兼容窗口期口径，按 `lan-no-pre-rename-compat` 记忆：改名前版本不兼容）。
- [ ] **SSH/WSL/Docker 远程 + workspaceIdentity**：身份 key `trim() || path` 贯穿，跨 Host 路由与 stale-run 防护的对拍测试。
- [ ] **Web 端（axum）**：`web-remote-replayable` 链路 + 手机远控复用 Host attachment 的既有架构（AGENTS.md：不为手机另起运行时）。
- [ ] **TUI（ratatui）**：对照旧 TUI 的会话/审批/流式场景清单逐项过。

**阶段验收：** 每个域有对照旧版的 E2E 清单且全绿；sidecar 断网降级符合预期；三平台安装包体积保持核心 ≤30MB。

---

## M4 自研验收 + UI clean-room 替换（2–3 个月）

**目标：** 删光上游衍生代码，"与上游逐文件 diff 清零"成为 CI 可跑的硬门禁，台账与 NOTICE 收口。

**Files:**
- Create: `packages/ui-next/`（或按 `DESIGN.md` 重建 `packages/ui` 替代品）、`scripts/upstream-overlap-check.mjs`（对 `upstream/zcode-baseline` 逐文件哈希对比 + 改名归一哈希对比，方法沿用 2026-10-09 评估的口径：逐字节一致 / 仅品牌归一一致 / 实质不同三档）
- Modify: `THIRD-PARTY-NOTICES.md`、`third-party/copied-components.json`（重建：删除全部"上游衍生"条目，保留第三方组件条目）、`NOTICE.md` §4（diff 清零后改写为"当前代码不含上游实现，历史版本的 Apache 义务见基线分支"——**仍保留 zai-org/ZCode 链接与历史声明，不删**）

**Interfaces:**
- Consumes: M1–M3 全部产出；`DESIGN.md`（作为 UI 重写的唯一规格）。
- Produces: `pnpm verify:self-developed`（= upstream-overlap-check + scancode/licensee 扫描 + 台账新鲜度），进 CI 必跑。

- [ ] **UI 替换**：按 `DESIGN.md` + 既有交互 spec 重写组件（渲染的是新 Tauri 前端，不回填旧 renderer）；逐屏与旧版截图对拍（允许视觉差异、不允许功能差异）。
- [ ] **重合度归零**：`upstream-overlap-check` 在 CI 断言"逐字节一致 + 仅归一一致"两档的行数占比 = 0%；首次运行产出的残余清单按模块排期清零。
- [ ] **第三方台账重建**：新依赖全量盘点（`scripts/licenses.mjs` 口径延续）；`copied-components.json` 中上游衍生条目删除、第三方条目（shadcn 思路自写组件、Material 图标去留、插件图标 reviewRequired）逐条复核。
- [ ] **法务收口**：贡献者协议（CLA/DCO）挂进 PR 流程；软著登记材料（源码前后 30 页 + 说明）；"Mode"图形组合商标申请材料；`docs/specs/p2-mode-naming.md` 必须保留清单逐条复核不动。

**阶段验收：** `verify:self-developed` 全绿（diff 0%）；UI 全量替换完成、旧 renderer 代码从产物中消失；第三方台账无 reviewRequired 中"上游衍生"类遗留；NOTICE §4 新措辞经人工复核。

---

## M5 发布与收口（1–2 个月）

**目标：** 新线 GA、旧线 EOL、三指标（语言/体积/重合度）落到终值。

- [ ] **发布工程**：Tauri updater + 签名（三平台）、`release.yml` 只出 Tauri 产物、块映射/差分更新验证。
- [ ] **旧线 EOL**：Electron 线打最后版本公告 EOL；`docs/plans` 与 spec 标注归档；仓库语言终态 = Rust + TS + HTML/CSS（目标 ≥99% 字节来自这两门 + 平台文件）。
- [ ] **数据迁移终验**：`.mode` 数据根原样复用（协议未变）；从旧线最后一版升级到新线的实机迁移演练。
- [ ] **三指标仪表入库**：`pnpm verify:self-developed`（重合度 0）、`scripts/check-language-policy.mjs`（语言）、CI 记录安装包体积（初装 ≤30MB 核心 + sidecar 按需）。

**阶段验收：** 三平台 GA 发布；三指标全绿并进必跑门禁。

---

## 风险与对冲

| 风险 | 对冲 |
|---|---|
| 行为对拍测不出隐性语义（双链路、owner/lease） | M2 对拍矩阵三层（工具序列/文件快照/事件流）+ 20 个真实任务盲跑；每改 stream/snapshot/queue 双链路双跑 |
| 插件/浏览器自动化等 TS 生态无法 Rust 化 | DR-01 sidecar 边界把它们挡在核心外；浏览器自动化（playwright-core 仅 TS）明确归 sidecar |
| clean-room 被 AI 提示词无意破坏 | 禁读规则写进 AGENTS.md 与任务模板；PR 检查清单 + M4 硬门禁兜底 |
| 工期失控（单人 18–30 个月） | 阶段门禁可停可续：每个 M 结束都是可发布检查点；旧线持续维护保住产品不失血 |
| 并行会话/多人同仓冲突 | pathspec 提交纪律（已入记忆）、新目录（`crates/`、`apps/desktop-tauri`）与旧栈零重叠 |

## 度量与预算

- **仪表三件**：语言占比（linguist + language-policy 脚本）、上游重合度（upstream-overlap-check）、安装包字节（CI 记录）。
- **预算**（沿用 2026-10-09 估算）：DeepSeek API 全程 ¥5k–3 万（flash 为主）；人力为大头：2–3 人 × 12–17 个月 ≈ ¥100–200 万，单人则为时间机会成本。
- **子计划节奏**：每阶段开工前 1 周内用 `writing-plans` 产出该阶段细粒度计划并过评审；本文件只在阶段边界变化时修订。
