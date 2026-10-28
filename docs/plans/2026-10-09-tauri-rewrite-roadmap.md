# 收窄语言 + Rust/Tauri 重写 + 产品自研：分阶段总体规划

> **For agentic workers:** 本文件是项目级路线图，不是单次可执行计划。每个阶段（M0–M5）在开工前必须按 `writing-plans` 技能单独产出一份细粒度子计划（含逐步骤、测试与提交点），存放到 `docs/plans/YYYY-MM-DD-<阶段名>.md`，本路线图只锁定阶段边界、验收口径与决策记录。

**Goal:** 把产品收敛为 TypeScript（前端/工具链）+ Rust（核心/平台）两门语言，用 Rust + Tauri v2 重写全栈，并在重写过程中以 clean-room 纪律实现与上游 `zai-org/ZCode` 的版权切分，达成真正的自研。

**Architecture:** 同仓库绞杀者演进——现有 Electron/TS 栈冻结为"参照实现 + 测试预言机"，新增 `crates/`（Rust workspace）与 `apps/desktop-tauri/`（Tauri v2 壳）；协议类型双端生成并对拍；插件生态以 Node sidecar 保留在核心之外（**Host sidecar 随包发布，插件/MCP/Skills 负载按需下载**，见 DR-M3-2）；体积按 DR-M5-2b 拆成「核心」与「Node 运行时 + sidecar」两份预算管理（原「初装 ≤30MB 核心 + sidecar 按需」口径已被 DR-M3-1/DR-M3-2 推翻）。

**Tech Stack:** Rust 1.8x / Tauri v2（tauri-plugin-updater、deep-link、os、tray）、tokio、axum、russh、ratatui、rmcp、portable-pty、notify、schemars/typeshare、React 18 + TS 5（存量前端）、Node sidecar（存量插件宿主）。

**Spec:** 每个阶段的实现 spec 在阶段开工时于 `docs/specs/` 新建；本路线图引用的既有真源：`AGENTS.md`（架构策略）、`DESIGN.md`（UI 规范）、`docs/specs/p2-mode-naming.md`（必须保留的合规段落）、`third-party/copied-components.json`（第三方台账）。

## Global Constraints

- **clean-room 纪律**：新代码只允许从本仓库 spec/协议/测试/公开文档出发编写；任何任务禁止阅读、引用、翻译 `zai-org/ZCode` 及本仓库中上游衍生的实现文件（例外：作为黑盒对拍的**运行行为**，不看实现）。
- **语言政策**：新代码只允许 Rust、TypeScript、HTML、CSS；shell 仅限 4 个既有胶水文件，新增需 spec 批准；`.agents/skills/**` 视为第三方 vendored，不参与语言收窄。存量 `.mjs`/`.js` 不做迁移；确需新增禁入扩展名的文件，经 `scripts/language-policy-allowlist.txt` 入清单（入清单 = spec 批准）。
- **不推送**：agent 只做本地提交；push 由人发起（push main 会触发自动发版，带 `[skip release]` 可跳过）。
- **合规不回退**：`LICENSE-APACHE`、`NOTICE.md` §4 衍生声明、`THIRD-PARTY-NOTICES.md` 台账在任何阶段不得削弱；M4 前所有发布物仍属 Apache-2.0 衍生作品，公告口径照实写。
- **三平台**：Windows x64、macOS arm64、Linux x64 全程 CI 出包；体积口径统一按 NSIS/Tauri NSIS 安装包字节数对比。
- **版本与工具**：Node 24.14.0、pnpm 10.33.2（`mise.toml` 为准）；Rust 工具链在 M0 钉版本（`rust-toolchain.toml`）〔M0 任务 6 经 DR-06 让行，未随首批落地——**M0 批次完成 ≠ M0 阶段关闭**，阶段关闭以任务 6 完成为准〕。
- **既有语义不得丢失**：owner/lease、CommandInbox 串行准入、`desktop-continuous` 与 `web-remote-replayable` 双链路语义、workspaceIdentity 贯穿——每阶段对拍清单必须覆盖。

## 三目标的合并逻辑（为什么合成一张路线图）

1. **语言收窄不在旧仓库全面执行**：旧栈 56% 是上游代码、终将被替换，在它上面做 122 个 `.mjs → .ts` 大迁移是消耗性工作。旧仓库只做两件零风险动作（vendored 标记、语言政策冻结），收窄在新代码里天然成立。
2. **自研通过重写实现**：重写即换血。只要全程 clean-room，M4 结束时"与上游逐文件 diff 清零"自然成立，不需要单独的清洗项目。
3. **体积由架构解决**：Tauri 核心无 Chromium；插件/MCP/Skills 负载按需下载（复用既有 `MODE_REMOTE_ASSET_CDN_BASE_URL`/`prepare:remote-assets` 基建）。**口径更正（DR-M3-2）**：Host sidecar（含 Node 运行时）是 UI 能渲染的前提，随安装包发布，不走按需下载；因此「Node 运行时不进初装包」不再成立，体积按 DR-M5-2b 的两份预算测量。

## 先拍板的五个决策（M0 决策记录，DR-01…DR-05）

| #     | 决策                        | 结论                                                                                                                                                                                                                                                                                                                     | 依据                                                                                                      |
| ----- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| DR-01 | 插件宿主                    | Rust 核心 + **Node sidecar 子进程**跑存量 TS 插件/MCP 工具/Skills；sidecar 首启按需下载〔**DR-M3-2 更正（2026-10-16）**：Host sidecar（含 Node 运行时）改为**随安装包发布**；按需下载仅保留插件/MCP/Skills **负载**，见 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`〕                                                                                                                                                                     | 插件生态是 npm/TS（272 插件目录、Claude/Codex 兼容），换引擎=放弃生态；插件属第三方代码，不在"自研"口径内 |
| DR-02 | 前端 UI 策略                | 骨架期（M1–M3）**可挂载现有 React UI** 加速联调（开发期不分发，不产生额外义务）；**M4 完成 clean-room 替换**，1.0 发布前删除全部上游衍生 UI                                                                                                                                                                              | `packages/ui` 有 15.8 万行仅改名上游代码，终态必须替换；但第一版就重写 UI 会拖死骨架进度                  |
| DR-03 | 上游同步                    | **立刻停掉** `upstream-audit.yml` 的定时同步（改 `workflow_dispatch`）                                                                                                                                                                                                                                                   | 继续同步=持续引入上游新版权，自研永远还不完                                                               |
| DR-04 | 上游基线                    | 把上游 main 导入为本仓库 `upstream/zcode-baseline` 分支（一次性 fetch + branch）                                                                                                                                                                                                                                         | 兑现 NOTICE"对照上游仓库 diff 追溯"；给 M4 的 diff 清零验收当标尺                                         |
| DR-05 | 仓库形态                    | **同仓库并存**（`crates/`、`apps/desktop-tauri/` 与旧栈同仓），旧栈打 EOL 后归档                                                                                                                                                                                                                                         | 复用协议定义、测试基线、CI 与第三方台账；避免双仓漂移                                                     |
| DR-06 | 与电脑控制（CUA）一期的归口 | 电脑控制并行会话已在做 **Rust addon（napi-rs + windows-rs）+ Rust 工具链接入 + electron-builder 打包**（`docs/specs/computer-use-windows-runtime.md`）——**Rust 工具链、cargo CI job、electron-builder 打包规则以先落地的一期为准**，M0 的 C# helper Rust 化跟进其约定；两者同期只允许一处改 `electron-builder.config.js` | 电脑控制一期实质上就是路线图要的"Rust 先行试水"，重复引入两套 Cargo workspace/CI 只会互相打架             |

---

## 总图（2–3 人 ≈ 12–17 个月；单人 ≈ 18–30 个月）

```
M0 地基与纪律   ▸ 2–4 周    语言冻结/合规基线/对拍机制/Rust 先行试水
M1 Tauri 骨架   ▸ 2–3 月    壳+平台服务层+协议双端+三平台出包
M2 运行时 Rust  ▸ 4–6 月    agent 会话/工具/权限/双链路语义
M3 生态层       ▸ 3–5 月    sidecar/插件市场/IM bot/LAN/远程/Web/TUI
M4 轻更快+验收  ▸ M4a 1–2w → M4b 2–4w  bench 基线/Host 并入壳 + UI 替换/diff 清零/台账/法务(并行)
M5 切换与收口   ▸ M5a 4–8w → M5b  Agent 切 Rust+Node 退场 + 签名/EOL/迁移终验/四仪表
```

---

## 进度快照（2026-10-27 更新）

> 证据：`docs/plans/2026-10-20-installed-0.0.61-acceptance.md`（已安装 0.0.61 自动化门禁与真机验收）、
> `docs/plans/2026-10-20-runtime-ui-fixes.md`（0.0.61→0.0.63 运行态修复）、
> `docs/plans/2026-10-19-lan-remote-sidecar-migration.md`（LR-0..LR-4 执行记录）、
> `docs/specs/legacy-electron-eol.md`（旧 Electron 线 EOL 执行记录）、
> `docs/plans/2026-10-25-installed-0.0.85-acceptance.md`（0.0.85→0.0.92 逐版验收；0.0.92 节为本快照输入）、
> `docs/plans/2026-10-27-0.0.92-acceptance-followups.md`（0.0.92 验收后改造项与判据）。

| 阶段 | 状态 | 已达成 | 仍阻塞阶段关闭 |
| --- | --- | --- | --- |
| M0 | 进行中 | 语言门禁、clean-room spec、上游基线、Rust 工具链、helper P1 与黑盒对拍 | `browser-import-helper` C# 重写与真实 Windows 会话验收（P2） |
| M1 | **已关闭（2026-10-20）** | v0.0.59 三平台出包首次全绿，v0.0.60/61 按三平台同发口径发布；10 命令 E2E、clippy/rustfmt、协议生成物 | 无；体积口径由 DR-M5-2b 放开，macOS/Linux 真机验收转入 M5 |
| M2 | 代码面完成，验收未过 | 工具/权限/会话流/对拍矩阵与 CI 门禁；Rust workspace 30 suites / 202 passed（2026-10-20 验收）；stdio 审批帧映射（2026-10-22，+13 用例） | 真实模型 20 个端到端任务未执行 |
| M3 | 进行中 | LR-0..LR-4 Host sidecar 接线；插件市场/IM/Web/TUI 复用既有实现；0.0.61 真机核心链路通过；**Host 自动重生（§22.4）、`exportLogs`（§27）（2026-10-26），半开管道心跳（§22.5）、终端 PTY 下沉到壳、通知点击聚焦（2026-10-27）**；0.0.92 真人操作验收核心链路通过 | 插件/MCP/Skills **负载**的 CDN 发布与产品接线（Host sidecar 已按 DR-M3-2 随包发布，不是缺口）；插件安装/卸载、IM 真账号、两台实机 LAN/RD/Web、TUI 交互验收；**杀 Host/心跳/失败屏 retry 真机用例（需外部会话）、远程工作区任务通知、出站 RD 首帧、exportLogs 体积口径** |
| M4 | 进行中 | UI 基础件 adapter 全量切换、业务代码对旧 `components/ui/*` 引用清零；上游重合度 9.17%（2026-10-24 复测） | 旧 UI 文件看屏验收与删除；重合度清零；Rust 台账 514 缺失项与法务收口 |
| M5 | 进行中 | 三平台 Release 发布口径已启用、Windows 自动发版连续跑通；旧 Electron 线代码/CI 已下线（`d71ed992`） | 三平台签名、EOL 公告与历史文档归档、差分更新、mac/Linux 真机安装/升级 |

> **范围外新增（2026-10-21 更新）**：`apps/remote-agent/`（Go，旧版 Windows 远程被控端，
> 规格 `docs/specs/legacy-windows-remote-agent.md`、计划
> `docs/plans/2026-10-20-legacy-windows-remote-agent.md`）已随 `8b64dcf5` 入库并通过
> Windows 10 x86/x64 与 MCP 客户端验收，XP SP3 实机待验。它不在 M0–M5 的 Rust/Tauri
> 范围内。`language-policy.md` 的允许语言未列 Go，但门禁脚本当前只拦
> `.py/.cs/.js/.mjs/.cjs`，因此 Go 文件能通过门禁；是否把 Go 写入允许语言或禁入清单，
> 仍待单独裁决（现状：不拦，属口径缺口）。

> **2026-10-26 追加**：自动化面又被推进三项并收口——`exportLogs`（LR-5 §27，命令第 15 行 +
> 单文件文本包 + 保存对话框）、Host 自动重生（spec §22.4，进程事实驱动 + 退避换代 + 整页
> reload + 终态屏 retry/exit 控制，真机杀进程 E2E 通过）、日志跨日滚动修复；另把
> `updater --lib` 的 `0xC0000139` 根因查明（测试清单指令非法，已修）。**仍未自动化的只剩
> 半开管道心跳（传输层独立立项）**；其余阻塞项与下方清单一致，均需人工/真机/真账号。
>
> **2026-10-27 追加（0.0.92 安装版真人操作验收）**：核心链路通过——启动/基础对话/
> 帮助菜单导出日志/托盘隐藏恢复/终端 PTY 父进程为壳/本机被控侧 CUA Helper。M3 的自动化面
> 本轮再收口三项：半开管道心跳（`cd2ea888`）、终端 PTY 下沉（`0f84ebbe`）、通知点击聚焦
> （`5a4a0f15`）；终端 shell 与 conhost 的父进程都是 `ModeTauri.exe`（PTY 下沉成立）。
> **三条破坏性用例已由脱离 Host 进程树的 WMI 编排器实测**：自然杀 Host 换代通过、
> 半开管道心跳 kill+换代通过、崩溃循环失败屏「重试」通过；但 Case A 暴露**终端跨整页
> reload 未存活**——reload 销毁渲染层 terminalId，现有实现没有 reattach/持久化，
> 已登记 P2-4（`terminal-pty-in-shell.md` 承诺与实现的缺口）；Case B 还暴露心跳 kill
> 后渲染层卡启动 Logo（Host 已 ready，Ctrl+R 无效，再杀一次 Host 才恢复），已登记
> P1-3。同轮代码修复：远程通知
> endpoint 反查 + 新终态会话通知（5 条单测）、`exportLogs` 只导 Host 线并输出 omitted
> manifest（12 条 crate 用例）、渲染层 typecheck 接入 root `typecheck` 并清零 19 个
> 存量错误（现为硬门禁）。
> 仍待人工/双机：出站 MX121212 远程桌面首帧、通知真机 toast/点击聚焦、P2-4 终端
> reattach。细化工作项、判据与 spec 落点见
> `docs/plans/2026-10-27-0.0.92-acceptance-followups.md`。
>
> **自动化推进收口（sess_9664bbd9，2026-10-23→24）**：本会话负责的可自动化面已清零。
> 本会话落地：LR-5 桩升级 13 条并收口登记（§26–27）、M4a 四件 bench 脚本与三件实测基线
> （§基线·IPC / 核心载荷 59.77 MiB / stream-latency 两段）、IM bot SDK 动态 import 半件、
> `KNOWN_V1_PAYLOAD_LAG` 两处协议 schema 漂移全部修复（`tauri-agent-runtime.md` §6.6）、
> 仪表刷新（重合度 9.17%）。**剩余均为人工项，无自动化可推进点**，逐项索引：
> 真机/真账号验收（M0 P2 UAC、M2 20 任务、M3 各域见快照表）、同意窗壳窗口决策（LR-3）、
> IM bot SDK 移出 ship 闭包的按需资产基建或
> fail-closed 拍板（M4a 可选项注记）、memory 30min 正式基线（等无实例窗口，脚本已就绪）、
> M4 并行子线人工段（UI 看屏/台账 Linux CI/法务）、M5 三平台签名与 EOL 公告。
> 复核入口：本节 + 快照表各阶段「仍阻塞」列。
>
> **2026-10-27 复核（对「无自动化可推进点」的修正）**：0.0.92 验收暴露出两项仍可自动化
> 推进的工程面——P0-1 外部编排脚本（绕开「验收会话自托管于目标 Host」的限制，自动跑
> 杀 Host/心跳/失败屏三条用例）与 P2-1 渲染层 typecheck 接入 CI（当前 19 个存量错误）。
> 二者见 `docs/plans/2026-10-27-0.0.92-acceptance-followups.md`。

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

- [x] **语言统计收窄**：`.gitattributes` 追加 `.agents/skills/** linguist-vendored`（它是 `copied-components.json` 台账在案的第三方复制件，语义成立）。效果：GitHub 语言条 C# 2.2%→0.2%、Python 0.6%→0、HTML 11.5%→0.1%（HTML 的 11.5% 实为 `xlsx/docs/cases/superstore…/superstore_orders_print.html` 单个 4.78MB fixture）。
- [x] **停上游同步（DR-03）**：`upstream-audit.yml` 删除 `on.schedule`，仅留 `workflow_dispatch`；提交信息注明决策号。
- [x] **导入上游基线（DR-04）**：`git fetch https://github.com/zai-org/ZCode.git main` → `git branch upstream/zcode-baseline FETCH_HEAD`（本地分支，由人决定何时推送）。
- [x] **clean-room spec**：新建 `docs/specs/clean-room-policy.md`，写明：禁读上游实现的适用范围（仓库内外所有新代码任务）、AI 提示词模板中的禁用引用、对拍只比行为不看实现、违规处置；`AGENTS.md` 增加一行指针。
- [x] **语言政策 + 门禁**：`docs/specs/language-policy.md` + `scripts/check-language-policy.mjs`——扫描 `git ls-files`（索引全量）+ `scripts/language-policy-allowlist.txt` 允许清单〔R2 裁定修正原计划的 `--cached` 口径：其在 pre-push/CI 接线点会空转〕，非 vendored 路径新增 `.py/.cs/.js`（`.mjs/.cjs` 亦拒，入清单=spec 批准）即退出 1；挂进 `pnpm verify:pre-push` 与 release CI。验收：暂存 `foo.py` 跑脚本 → FAIL；移除 → PASS。〔下一批候选：真 `.husky/pre-push` 钩子 vs spec 明写「约定执行 + release CI 硬兜底」二选一（终审 Minor#3）；`check-language-policy.mjs` 对 git 不可用/清单缺失的异常文案打磨（E-1）〕
- [x] **Rust 工具链钉版（2026-10-15 落地）**：根 `rust-toolchain.toml` 确立为单一真源，`channel = "stable"` + `components = ["clippy","rustfmt"]`，与电脑控制一期 CI（`release.yml` 的 `dtolnay/rust-toolchain@stable`）对齐。刻意不声明 `targets`：本地 CUA helper 默认走 gnu 交叉目标（`scripts/build-cua-helper.mjs` 的 `MODE_CUA_AX_TARGET`），写死 Windows 目标会让 mac/linux CI 腿装无用交叉 std；精确小版本待 M1 定下交叉目标口径后再收紧。
- [ ] **`crates/browser-import-helper` 重写 C#（M0 关闭条件）**〔2026-10-15 补齐计划与契约〕：**新发现的纪律约束**——`Program.cs` 与 `upstream/zcode-baseline` 逐文件 diff **只差 1 行**（1 insertion / 1 deletion），属 `clean-room-policy.md` 口径下的上游衍生实现文件，**禁读**；「先写行为对拍测试（同输入 → 与 C# 版输出一致）」必须按**黑盒**执行，不得打开旧实现照着写。
  - 契约真源：`docs/specs/browser-import-helper-contract.md`（黑盒实测 + 接口面：`--version` 五字段横幅、`--broker --parent-pid` 的 tab 分隔 3 字段请求/响应、宿主 reason 白名单 13 项、90s 超时与 128KiB stdout 上限）。
  - 执行计划：`docs/plans/2026-10-15-browser-import-helper-rust-port.md`（P1 纯逻辑可本机验，P2 提权/服务通道/密钥解封需真实 Windows 会话，P3 接线删 C#）。
  - 本机前置已验：无 dotnet/csc，但 `C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe` 可用，`pnpm --filter @mode/desktop prepare:browser-import-helper:oracle` 能产出 oracle（2026-10-16 起：`prepare:browser-import-helper` 已改指 Rust 构建，C# 的构建入口挪到带 `:oracle` 后缀的脚本）。
  - **实测发现的现役缺陷（2026-10-15）**：旧 C# 产物对 `--version` 横幅与 broker 回帧**都发 `ZCODE_*` 品牌**，而宿主断言 `MODE_BROWSER_IMPORT_HELPER` / `MODE_BROWSER_IMPORT_V1`——两者不符。补充核实：已发布的 Mode 安装件 `app.asar` 里**只有 `MODE_*`**、且 `resources/` 下**没有 browser-import 目录**，即该 helper 未随本仓库的发布件一起分发；所以这是**代码库内的一致性缺陷**（一旦启用就会失败），不是线上事故。Rust 版按宿主契约实现（`rejects_legacy_zcode_token` / `responses_never_use_the_version_token` 反向锁），替换 C# 即修复。
  - **解封面 spike 结论（2026-10-15）**：机制是 Chrome 的 COM elevator（`IElevator2::DecryptData`，路径校验面向浏览器进程）。关键否证：**旧产物在本机（Chrome 155 已安装并运行）用真实 app-bound 密钥也只回 `broker_initialization_failed`**，成功路径没有参照物。详见 `docs/plans/2026-10-15-browser-import-abe-spike.md`；验收改自证口径（解开真实 cookie 密文），且**若结论指向进程注入则需独立立项与安全评审**。
  - **解封路径需真机**（本机无 Chrome、需 UAC 交互式会话），故 **C# 暂不删除**：删早了会让该功能直接不可用。删除条件是 P2 落地并在真机跑通「导入一次 + 拒绝 UAC」两条验收。
  - 验收：Windows 实机导入一次浏览器数据成功（含拒绝 UAC → `chrome_cookie_elevation_cancelled`）；语言条 C# 归零。
  - **2026-10-16 进展（P1 收口 + 构建接线；阶段仍未关闭）**：
    1. **「本机无 Rust 链接器」的结论作废**——电脑控制一期落地时已在本机装配出 gnu 路线工具链（`~/.rust-tools/llvm-mingw-*/bin/x86_64-w64-mingw32-clang` 作 linker + rustup `stable-x86_64-pc-windows-gnu`），`scripts/build-cua-helper.mjs` 一直在用它。实测本机 `cargo test` **21 个用例全绿**、`clippy --all-targets -- -D warnings` **零告警**；无需装 VS Build Tools，**不构成环境变更**。
    2. **构建接线落地**：新增 `scripts/build-browser-import-helper.mjs`（固化 gnu 工具链装配、产物自检、非 win32 平台 skip、`MODE_BROWSER_IMPORT_TARGET` 可覆盖），`prepare:browser-import-helper` 改指它；本机实测产出 `bundled-tools/win32-x64/browser-import/mode-browser-import-helper.exe`。旧 C# 脚本**不删**，改挂 `prepare:browser-import-helper:oracle`，以便继续复现旧产物做黑盒对拍。
    3. **黑盒对拍闭环**：Rust 与旧产物在「协议版本 + 架构 + 应用版本」3 字段一致；第 1 字段按设计**不同**（旧产物 `ZCODE_*`、Rust `MODE_*`，正是要修的品牌缺陷）。契约测试 5 passed（含对拍用例）。落点冲突已处理：两个产物共用 `bundled-tools/win32-x64/browser-import/`，对拍旧产物须显式给 `MODE_BROWSER_IMPORT_ORACLE_PATH`，未指定时对拍用例整条 skip（不给假绿）。
    4. **仍未完成**：P2（提权 / 服务通道 / 密钥解封）需要真实 Windows 交互式会话做 UAC 验收，无人值守执行会弹窗挂住；故 `Program.cs` 与其语言政策 allowlist 行保留、**C# 语言条未归零**，`electron-builder.config.js` 的 browser-import `extraResources` 接线与打包态 Authenticode 校验一并留到 P3（本轮刻意不动该文件，遵守 Global Constraints 的「同一窗口期只允许一处改动」）。**M0 仍以本任务未完成论**。
    - **2026-10-17 更新（用户选 B）**：A 参照已在宿主链路跑出（C# oracle → `ERR decryption_failed`；
      本机提权静默、无 UAC 弹窗），成功路径仍不存在。P2 移交独立 spike
      `docs/plans/2026-10-17-chrome-abe-import-spike.md`，安全门禁
      `docs/specs/chrome-abe-import-security.md`；C# 保留、Rust 停在 P1、C# 归零继续挂起。
- [ ] **mode-cua JS→TS（让行）**〔本批让行，待电脑控制一期收口〕：等电脑控制一期收口后再执行（该目录正在被并行会话开发）；`.d.ts` 手写声明删除，改由 tsc 产出。
- [ ] **（可选）过渡版瘦身**〔本批未执行〕：按上轮结论修 `files` 白名单（echarts/mermaid/pdfjs/lucide/shiki/katex 等渲染层库已在 `out/renderer` bundle 内，asar 不再重复装其 node_modules）+ `compression: "max"`，本地出包对比体积并记录。**与 CUA 一期对 `electron-builder.config.js` 的改动错峰，同一窗口期只动一处。**

**阶段验收：** 语言政策门禁在 CI 生效；`upstream/zcode-baseline` 存在；clean-room spec 合入；Rust helper 替换 C# 后三平台包可出〔此项属任务 6，经 DR-06 让行后置〕；GitHub 语言条只剩 TS/TSX + JS（旧脚本）+ HTML/CSS。〔首批已验：门禁生效、基线存在、spec 合入；2026-10-15 追加：`rust-toolchain.toml` 钉版落地、helper 重写计划与黑盒契约 spec 就位；**阶段关闭待任务 6 的 C# 重写与实机验收**〕

---

## M1 Tauri 骨架（2–3 个月）

**目标：** Tauri v2 壳跑通存量 React UI 与平台服务层，三平台出包，初装体积出现数量级下降。

**子计划：** `docs/plans/2026-10-15-tauri-skeleton.md`（2026-10-15 产出，开工即用；含命令切片表、事件通道、capability 规则、体积基线与风险）。

**Files:**

- Create: `apps/desktop-tauri/`（`src-tauri/`：taui v2 工程 + `Cargo.toml` workspace 根挂 `crates/*`）、`crates/platform/`（IPlatformService 的 Rust 对应）、`crates/protocol/`（schemars + typeshare）、`crates/updater/`、`crates/tray/`、`crates/deeplink/`
- Modify: `pnpm-workspace.yaml`（纳入 `apps/desktop-tauri`）、`.github/workflows/release.yml`（新增 tauri 出包 job）、`docs/specs/tauri-platform-commands.md`（本阶段 spec，开工时先写）

**Interfaces:**

- Consumes: 存量 `packages/ui`（DR-02 挂载）、`packages/shared/src/platform.ts` 的 `IPlatformService` 方法清单（作为 Rust 命令面的**接口规格**，只读签名与文档，不读实现）。
- Produces: `platform::commands`（`#[tauri::command]`，与 `IPlatformService` 逐方法同名同参）、`protocol` crate 导出的 TS 类型（`typeshare` 生成 `packages/shared/src/generated/tauri-protocol.ts`）、`ModeTauri.exe` 安装包（≤25MB 目标）。

- [x] **脚手架**（2026-10-16 落地）：**Deviation**：没用 `cargo create-tauri-app` 落 react-ts 模板——骨架只需要一个空窗口，手写 `tauri.conf.json` + `main.rs` 更可控（模板会额外带一套前端构建链，与 DR-02「挂载存量 UI」冲突）。根 `Cargo.toml` workspace 显式列举成员并 `exclude = ["crates/mode-cua-ax"]`；CI 加 `cargo test -p platform -p protocol` + `clippy --all-targets -D warnings`。
- [x] **平台命令面**（2026-10-16，10 条注册可调用）：切片表由 `command_slice!` 宏声明为唯一真源，`init()` 与 `registered_names()` 从它派生，另有「JS 名 = 处理函数名 camelCase」守卫。**两处部分实现**（spec §5 登记）：`getApplicationIcon` 一律返回 `null`（真实取值要按平台做原生图标解码）；`saveFile` 的 `sourceUrl` 分支明确返回 `not_implemented_until_m3` 而不是静默写空文件。原计划列的「剪贴板 / 托盘 / 深链 / 代理设置 / 更新检查」不在 10 条切片内，留 Task 6 与 M3。
- [x] **协议双端**（2026-10-16 落地）：Rust 侧真源 `crates/protocol`（8 个类型，全部 `rename_all = "camelCase"`，含 `SaveFileRequest` 的「data 与 sourceUrl 恰有其一」不变量）+ 往返测试 6 用例。**Deviation**：没用 typeshare，改走 `crates/protocol/src/bin/emit-schema.rs`（schemars → JSON Schema）+ `scripts/generate-tauri-protocol.mjs`（→ `packages/shared/src/generated/tauri-protocol.ts`），`--check` 已接进 CI，避免额外安装 CLI。生成器有一条约定：可选字段去掉 schemars 给的 `null` 分支（对应 Rust 侧 `skip_serializing_if`），产物因此与 `platform.ts` 手写声明一致。
- [x] **UI 挂载（DR-02）**〔2026-10-16 落地，**Deviation**〕：适配层
      `packages/ui/src/platform/tauri-invoke-shim.ts` 已落地（15 用例全绿，含编译期穷尽检查——
      协议新增必选方法而 shim 没跟上时 typecheck 报错），并从 `@mode/ui` 的 `./tauri-platform`
      子路径对壳公开。**挂载本身改成命令面验收台**：计划原文的「`frontendDist` 指向 renderer 产物」
      实测做不到——存量 renderer 的启动链要求 Node Host 经 MessagePort 提供 `IServiceAccessor`
      与数据库启动通道，那正是 M3 的 sidecar（DR-01）；没有 Host 只会停在启动/失败屏。
      证据表见 `docs/specs/tauri-platform-commands.md` §4.2，产品 UI 挂载登记到 M3。
- [x] **三平台出包**〔2026-10-16〕：release.yml 新增 `tauri` job（win/msvc+nsis、
      mac-14/arm64+dmg、ubuntu-24.04/x64+appimage），产物名 `Mode-<版本>-<平台>-<架构>.<ext>`。
      **产物只发 workflow artifacts、不进 Release**（与旧线同名会互相覆盖），也**不挂 publish 门禁**
      （三条腿未本机验证）——两处偏差与转正条件写在 workflow 注释与 spec §8。
      体积基线：Windows NSIS **7.19 MiB**（实测，未含产品 UI 资产），mac/linux 待首次 CI。
- [x] **更新链 stub**〔2026-10-16〕：`tauri-plugin-updater` 接现有 GitHub Releases feed
      （与旧线共用 `eafenzhang/Mode`、版本号延续），只声明 `latest.json` 端点 + 注册插件；
      不注册更新命令、不设 pubkey（让 `check()` 明确失败而不是静默放行未签名更新）。

**阶段验收：** 三平台安装包全绿且 Windows ≤30MB（争取 25MB；renderer 资产 53MB 解包口径是主要变量）；Tauri 版可完成"打开窗口 → 渲染 UI → 走通 10 个平台命令"的 E2E；`cargo clippy -D warnings` 零告警；不带任何 Node 运行时。〔骨架态成立；产品线按 DR-M3-1/DR-M3-2 随包带 Node，见下方 2026-10-20 阶段关闭记录。〕

〔**2026-10-16 进度（第二批后）**：Task 1–6 的代码与自动化都已经落地，
本机实测 `cargo test` **35 用例全绿**（platform 3 / protocol 6+5 / deeplink 7+9 /
tray 1 / updater 2 / tauri-test-manifest 2）、`clippy -D warnings` 零告警、`rustfmt --check`
干净、`cargo build -p mode-tauri` 与 `tauri build --bundles nsis` 均产出产物
（NSIS 安装包 7.19 MiB）、`pnpm typecheck`（含壳前端）通过、协议生成物 `--check` 无漂移、
语言政策门禁通过、release.yml 经 YAML 解析确认三平台矩阵。

**M1 关闭前的最后一件（2026-10-20 已消除）**：

1. ~~**Task 4 Step 3 的人工 E2E**~~〔**2026-10-17 已执行**：10 命令 + 深链逐条核对，
   记录见 spec §4.4 与 `docs/plans/2026-10-15-tauri-skeleton.md` 的「第四批」；
   验收台补了 `saveFile(sourceUrl)` 入口，成功/取消/超限/超时四条分支均已核对〕。
2. ~~**macOS / Linux 两条出包腿的首次 CI 运行**~~〔**2026-10-20 回填**：v0.0.59 的 CI
   首次同时产出 macOS arm64 dmg 与 Linux x64 AppImage，M1 登记的两条构建风险实测收口；
   `248b0937`（v0.0.60）移除两条腿的 `continue-on-error`，三平台安装包与各自 `sha256`
   片段一起上传同一 Release；v0.0.60 / v0.0.61 已按该口径发布〕。

**两处口径差异（2026-10-19 已由人裁定并落地）**：

- ~~Tauri 产物只发 workflow artifacts~~：`248b0937` 起三平台安装包与各自 `sha256` 一起进同一
  Release；旧 Electron `desktop` job 当时继续只发 workflow artifact（该 job 已于
  `d71ed992` 随旧线 EOL 删除，见 `docs/specs/legacy-electron-eol.md`）。
- ~~`tauri` job 没挂进 `publish` 的 `needs`~~：`248b0937` 移除两条非 Windows 腿的
  `continue-on-error`，任一腿失败即保持 draft；首次三腿全绿发生在 v0.0.59。

详细记录见 `docs/plans/2026-10-15-tauri-skeleton.md` 的两份「2026-10-16 执行记录」。〕

〔**2026-10-20 阶段关闭**：M1 验收中的「三平台安装包全绿」「10 命令 E2E」
「`cargo clippy -D warnings` 零告警」均已满足。两处口径按后续决策更新：① 体积不再作为
阻断门禁，按 DR-M5-2b 登记——骨架态 NSIS 7.19 MiB，当前含产品 UI / Host sidecar 的整包
51.50 MiB、核心 23.29 MiB；② macOS/Linux 只验证了构建与出包，真机安装/启动/升级验收转入
M5 残余风险。M1 的 Tauri 壳本身不依赖系统 Node。**口径更正（DR-M3-2）**：Host sidecar
（含 Node/agent 运行时）按决策**随安装包发布**，因此「整包不带任何 Node 运行时」不会在产品线
成立；该项不再作为 M3 跟踪项，体积按 DR-M5-2b 的两份预算持续测量。M1 阶段关闭不等于三平台 GA。〕

---

## M2 Agent 运行时 Rust 化（4–6 个月）

**目标：** 把会话循环、工具执行、权限引擎、双链路语义搬到 Rust，且与旧 TS 运行时**行为对拍**通过。

**子计划：** `docs/plans/2026-10-16-tauri-agent-runtime.md`（2026-10-16 产出，含四项决策记录
DR-07…DR-10、任务切分、对拍 oracle 契约与风险对冲；Task 0–2 已到逐步骤可执行，
Task 3–7 开工时各出子计划）。**开工前先读它的「与路线图 M2 节的口径修正」一节**：
本节的工具名清单与实际注册表不符（见下）。

**Files:**

- Create: `crates/agent-core/`（session loop、streaming、CommandInbox、owner/lease、权限引擎）、`crates/agent-tools/`（fs/git/shell/pty[portable-pty]/browser 桥）、`crates/agent-protocol/`（stdio/WS 帧，对应 `packages/shared/src/mode-protocol`）、`docs/specs/tauri-agent-runtime.md`
- Test: `crates/agent-core/tests/`（对拍套件）、复用 `pnpm --filter @mode/services test` 作为行为规格

**Interfaces:**

- Consumes: `crates/protocol`（M1）、`crates/platform`（工具要弹审批 UI 走平台命令）。
- Produces: `agent_core::runtime::Runtime::run(prompt) -> Stream<Event>`、`agent_tools::registry()` 工具声明表（name/schema/权限等级，供权限引擎与 UI 审批用）。
  〔口径修正〕原写的「typeshare 导出」不成立：**运行时协议在 M2 期间仍以 TS 为真源**
  （DR-07）——旧 TS 运行时此刻还在现网跑并充当对拍 oracle，把真源翻到 Rust 会让「对拍」
  退化成「自己对自己」。做法改成 `crates/agent-protocol` 做 TS 的镜像，用**共享 JSON 夹具
  两端同断言**；真源翻转是 M4/M5 删掉 TS 运行时时的事。另：M1 也没用 typeshare（改走
  schemars + 仓库内生成脚本），这一行原文是与 M1 实际产物的另一处不一致。

- [x] **只读工具先行**〔口径修正，2026-10-18 按实测回填〕：`Glob`/`Grep` 在 oracle 运行时
      **未注册**（调用返回 `tool_not_found`，见 spec §3.2 与 Ruling 17），因此实现范围实际是
      `Read` + 「未注册工具不产生 `started`」的事件形状；对拍用例在
      `crates/agent-tools/tests` 与 `crates/agent-core/tests/tool_parity.rs`。
- [x] **文件写与 diff 呈现**〔2026-10-18 收口〕：`Write`/`Edit` 与审批已落地
      （`approval.rs`、`permission_flow.rs`，`ApplyPatch` 实测不存在、已作废）；
      `display.file_diff`（`crates/agent-tools/src/file_diff.rs`）已补，`Write` 覆盖与 `Edit`
      成功结果带 diff、新建 `Write` 不带（观察）；`tool_parity` 逐字段比较 `display`。
- [x] **shell 与 pty**〔2026-10-18 收口〕：Bash 与超时/取消/退出码对拍已落地；oracle 实测
      管道而非 TTY（`[ -t 1 ]` 为假），**不引入 `portable-pty`**；host 层 `CommandInbox`
      （`crates/agent-core/src/command_inbox.rs`，同 key FIFO、stale 完成不改队列、无超时）
      已按 DR-T4-7 (c) 落地并由 5 条用例钉住顺序。Host 实际接线在 M3/M4，跨层对拍待 host 面可观察。
- [x] **会话流与重放**〔2026-10-18 收口〕：`desktop-continuous` 用
      `crates/agent-core/tests/stream_parity.rs`；`web-remote-replayable` 的 `deliveryKind`
      透传由 `turn_stream.rs` 锁定，重放语义归 Host（DR-T5-1 (a)），由
      `packages/desktop/tests/task-realtime-replay.test.mts` 覆盖；改 stream/snapshot/queue 时两侧双跑。
- [x] **权限引擎**〔2026-10-18 按实测回填〕：`permission_engine.rs` / `permission_channel.rs` /
      `approval.rs` / `rule_store.rs`；`allow_project` 落 `.mode/cli/db/db.sqlite` 的
      `local_setting`（D1 裁决「Rust 直连 sqlite」，含并发追加与丢失更新用例）。
- [x] **对拍矩阵收口**〔2026-10-18 按实测改口径〕：`packages/services/test` 60 个文件里
      **0 个**引用运行时会话面，原「198 个测试里挑运行时用例」的前提已过期；覆盖来源改为
      oracle 夹具 + `crates/agent-*/tests`，分类与理由见 `docs/specs/agent-runtime-parity-matrix.md`。
      CI 门禁 `scripts/agent-runtime-parity-matrix.ts --check` 已挂进 release `verify`
      （60/60 归类、11/11 资产存在）。真实模型 20 个端到端任务仍属阶段验收第 3 条，未执行。

〔**2026-10-20 回填**：0.0.61 验收时 `node scripts/rust-cargo.ts test --workspace --locked`
为 30 suites / 202 passed / 0 failed，对拍矩阵 CI 门禁稳定。阶段验收第 3 条「真实模型 20 个
端到端任务」仍未执行（需人参与与计费），**M2 不能关闭**。
同日复跑补充：本机 gnu 路线下 `updater --lib` 与 `mode-tauri --lib` 的测试二进制启动失败
（`0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND`），其余 crate 测试全绿；疑为本机 WebView2/DLL
加载环境问题，需在 CI/MSVC 复核，不能据此判定 Rust 代码回归。
〔**2026-10-26 收口**：`updater --lib` 的 `0xc0000139` 根因查明并修复（证据链见
`docs/plans/2026-10-25-installed-0.0.85-acceptance.md`「updater 0xC0000139 收口」）：
`tauri-test-manifest` 发的 `cargo:rustc-link-arg-tests` **不是 stable cargo（1.99 全系）
的合法指令**——全新构建直接 `invalid instruction` 硬失败，target 缓存回放时被**静默忽略**，
GNU 测试二进制因此从未带上 comctl32 v6 清单，运行即 0xC0000139（导入表精确比对定位到
`TaskDialogIndirect` 只在 v6 导出）。CI 绿是 rust-cache 命中旧产物的假象（win 腿零编译）。
已改用合法的 `cargo:rustc-link-arg`（不向依赖方传播，防 app 清单撞车的设计不变），
updater/platform/deeplink 三包测试本机首次全绿。`mode-tauri --lib` 的
web_assets_extraction 仍维持 CI/MSVC 权威口径。〕
〔**2026-10-22 回填**：Task 6 Step 5 已落地——`FramePermissionHandler`（`agent-core`）与
`agent-protocol::permission` 的帧编解码把 channel 两端接到 `interaction/requestPermission`；
新增 13 条用例（协议 6 + 会话 7）全绿，`clippy -D warnings` 与 rustfmt 通过。M2 剩余代码项
清零，**只剩真实模型 20 个端到端任务这一条人工验收**。〕

**阶段验收：** 对拍套件全绿（工具序列 + 文件快照 + 事件流三层一致）；双链路 E2E 各自通过；用真实模型跑通 20 个端到端任务（改代码/跑测试/浏览器）无行为回归。

---

## M3 生态层（3–5 个月）

**目标：** 插件 sidecar、市场、IM bot、LAN 远连、SSH/WSL/Docker 远程、Web 端、TUI 全部落到新栈。

> **生态域子计划（2026-10-18）**：插件/MCP/Skills 宿主见
> `docs/plans/2026-10-18-m3-plugin-host.md`（含 DR-PH-1…5 与 Task 0 观察清单）；
> LAN / 远程 / 远程桌面的验收现状见 `docs/plans/2026-10-18-m3-lan-remote-status.md`
> （结论：由 Host sidecar 复用，38 条相关测试全绿，待两台机器人工验收）；
> **Tauri 产品壳的接线缺口与迁移计划**见 `docs/plans/2026-10-19-lan-remote-sidecar-migration.md`
> （2026-10-19 独立立项；旧文「不是代码缺口」只适用于 Electron 服务层）；
> IM bot 见 `docs/plans/2026-10-18-m3-im-bot-status.md`（DR-IM-1：由 Host sidecar 复用，
> 149 条现有测试全绿，M3 不新增 `crates/bots`；剩真账号逐通道人工验收）；
> 插件市场见 `docs/plans/2026-10-18-m3-plugin-market-status.md`（DR-PM-1：sidecar 复用，
> 32 条服务测试 + 5 条卡片投影测试全绿，M3 不新增 `crates/marketplace`）；
> Web/手机远控见 `docs/plans/2026-10-18-m3-web-remote-status.md`（DR-WEB-1：sidecar 复用，
> 156 条测试全绿，M3 不新增 `crates/server`）；
> TUI 见 `docs/plans/2026-10-18-m3-tui-status.md`（DR-TUI-1：沿用 TS 客户端，M3 不新增
> `crates/tui`；33 个场景仅有编译面证据，交互验收待人工）。

**Files:**

- Create: `crates/plugin-host/`（sidecar 生命周期管理：按需下载/启动/健康）、`crates/marketplace/`（`.mode-plugin` 清单与市场目录，兼容读旧格式）、`crates/lan/`（UDP 发现 + 配对码）、`crates/remote/`（russh / 系统 ssh 调用 + workspace attach）、`crates/server/`（axum，web-remote 链路）、`crates/tui/`（ratatui）、`crates/bots/`（IM 桥：HTTP/WS 直连或 sidecar 复用 TS SDK，按通道逐个决策）
- Modify: `apps/mode-cli`（旧 TUI 标记归档）、`docs/specs/` 对应各域 spec（沿用既有 `lan-*`、`remote-workspace-identity`、`plugin-store-availability` 等 spec 作为行为规格）

**Interfaces:**

- Consumes: `agent_core::runtime`（插件工具注入点）、`crates/marketplace`（M2 的工具注册表）。
- Produces: `plugin_host::AssetCache::ensure()`（返回 bundle 路径，缺失则按 manifest 下载、校验 sha256、缓存）、`plugin_host::supervisor`（进程监督）、`server::app()`（axum Router，路由与旧 server 一一对应，契约测试对拍）。

- [ ] **插件宿主负载按需下载（Host sidecar 随包）**：按 DR-01 + DR-M3-2 —— Host sidecar
      （含 Node 运行时与 `agent/glm`）**随安装包发布**，保证全新安装 + 断网也能渲染 UI；
      按需下载只针对插件/MCP/Skills **宿主负载**：bundle 作为独立资产上 CDN，
      `plugin_host::AssetCache::ensure()` 首启用时下载、校验 sha256、健康探测；本地开发直连不下载。
      验收：断网时核心功能全在，仅插件功能提示"需下载组件"。〔2026-10-19 核对：M3 Task 1–5 的自动化面已完成——`crates/sidecar-host`、`host-standalone`、`window.mode` 注入、平台缺口（单实例/图标/sourceUrl）、Windows 安装包与两份体积预算均有测试/原始输出；`apps/desktop-tauri` 实测 35 pass / 7 opt-in skip。剩余是插件负载的 CDN 发布/真实 `assetUrl` 填充与真机人工验收；下载/校验/缓存逻辑已由 `crates/plugin-host` 实现并有本地 HTTP 用例。〕
      〔2026-10-19 补记：v0.0.57 真机验收发现安装包未带 agent 运行时，发送任务与插件市场均失败；`afb49a4c` 把 `agent/glm` 纳入发运集并由 Host 注入启动命令，v0.0.58 本机复验任务发送与插件市场通过。〕
      〔2026-10-20 回填：0.0.61 真机复验任务发送、插件市场目录、LAN 状态/配对码/发现均通过；
      当前发运形态是 Host sidecar + Node + `agent/glm` **随包**（符合 DR-M3-2）；仍未实现的是
      插件/MCP/Skills 负载的 CDN 发布与产品接线。初装包含 Node 是决策结果，不是缺陷。〕
- [ ] **插件市场迁移**：读旧 `installed_plugins.json`/marketplace 缓存原样可用；市场卡片 E2E 复用现有 `plugin-store-availability` spec 场景。〔2026-10-18 盘点，DR-PM-1〕读兼容与卡片投影已覆盖（32 + 5 条测试全绿），**M3 不新增 `crates/marketplace`**；剩真实窗口 UI E2E 与插件负载的 CDN 发布/产品接线，详见 `2026-10-18-m3-plugin-market-status.md`。〔2026-10-19：v0.0.58 真机复验市场目录可正常加载（旧错误已随 agent 随包修复消失）；安装/卸载动作尚未逐项执行。〕
      〔2026-10-20：0.0.61 市场页加载出完整插件卡片与「安装」按钮；安装/卸载动作仍未逐项
      执行，真实窗口 UI E2E 未完成。〕
- [ ] **IM bot 逐通道**：〔2026-10-18 盘点，DR-IM-1〕由 Host sidecar 复用现有实现（149 条测试全绿），
      **M3 不新增 `crates/bots`**；Rust HTTP+WS 重写延后到运行时移出 sidecar 时再评估。
      剩余为飞书/企微/钉钉/微信的真账号人工验收，逐项见
      `docs/plans/2026-10-18-m3-im-bot-status.md`。
      〔2026-10-20：A+ 改造已落地（单实例全局有界并发、群授权管理、出站会话级队列、
      Agent 归属只读投影），Webhook HTTP 入口与 Telegram 运行时已移除；四通道真账号
      验收仍未执行。〕
- [ ] **LAN 远连**：UDP 广播发现 + 6 位配对码 + 桌面即服务端——对照 `lan-discovery/lan-remote-attach/lan-paired-devices` 三个 spec 逐条验收（含旧客户端兼容窗口期口径，按 `lan-no-pre-rename-compat` 记忆：改名前版本不兼容）。〔2026-10-18 已跑 38 条相关测试全绿；2026-10-19 立项补 Tauri 产品壳接线，见 `docs/plans/2026-10-19-lan-remote-sidecar-migration.md`；两台机器验收在迁移后执行〕
      〔2026-10-20 回填：LR-0..LR-4 的 Host sidecar 接线已随 `bd4c7e46` 落地（LAN 管理面、
      远程桌面设置/会话、远程工作区 connect/attach、scoped attachment 多路复用）。
      0.0.61 真机验证 LAN 监听、配对码、发现与已配对设备通过。远程桌面「对端不支持」的
      根因已更正：不是 `mediaTicket`，而是 Tauri 包未带 Windows CUA helper
      （`rd_media_start` → `helper_unavailable` → 对端回 `unsupported`）；`a2fdebf6` 已把
      helper 纳入发运集/安装包并桥接 sidecar 资源目录，待真机复验。远程工作区设置页仍无
      attach/打开入口，两台实机端到端验收未完成。〕
      〔2026-10-20 追加修复：`13195643` 修复远程工作区断链卡死（WS close 接线 + 30s 心跳）
      与 LAN 端口占用导致 Host 启动失败（EADDRINUSE 改为 rejection）；`a2fdebf6` 修复 CUA
      helper 随包；`e5d78685` 修复 Tabs adapter 根节点布局；`a30b08c5` / `424d92fc` 修复
      机器人接入、回复模式、设置面板布局与下拉选择。以上均为 M3/M4 运行态修复，不改变阶段边界。〕
      〔2026-10-25 追加修复：`d5ba0671` Host 传输死亡渲染层 fail-closed 收口——0.0.85 真机验收
      实测 RD 会话异常结束后窗口全部服务 RPC 永久 pending（根因：main.tsx 启动完成后丢弃
      状态流，ChannelClient 永不 dispose）；现在 post-boot failed 会终止 base/scoped 连接并切
      启动失败终态。口径与开放项见 `docs/specs/tauri-renderer-host.md` §22。〕
- [ ] **SSH/WSL/Docker 远程 + workspaceIdentity**：身份 key `trim() || path` 贯穿，跨 Host 路由与 stale-run 防护的对拍测试。〔2026-10-18 相关用例已在 40 条 desktop 远程测试中通过（含判等口径与对新对端归属）；Tauri 产品壳的 `connectRemote` 与 scoped attachment 尚未接线，纳入 `docs/plans/2026-10-19-lan-remote-sidecar-migration.md`〕
      〔2026-10-20：`connectRemote` 与 scoped attachment 已随 LR-4 接线（LAN 主路径）；
      Tauri Host 对 SSH/WSL/Docker 明确返回「暂不支持」，未伪装成功，SSH/WSL/Docker
      仍属后续工作。〕
- [ ] **Web 端（axum）**：`web-remote-replayable` 链路 + 手机远控复用 Host attachment 的既有架构（AGENTS.md：不为手机另起运行时）。〔2026-10-18 盘点，DR-WEB-1〕由 Host sidecar 复用现有 `packages/server`，**M3 不新增 `crates/server`**；156 条相关测试全绿，剩两台设备人工验收，见 `2026-10-18-m3-web-remote-status.md`。〔2026-10-20：0.0.61 移动端远程控制页仅验证了四个 IM 通道的占位 UI；两台设备 Web/手机远控回归仍未执行。〕
- [ ] **TUI（ratatui）**：对照旧 TUI 的会话/审批/流式场景清单逐项过。〔2026-10-18 盘点，DR-TUI-1〕沿用现有 TS/React TUI（`@mode/tui`），**M3 不新增 `crates/tui`**；33 个场景清单见 `2026-10-18-m3-tui-status.md`；当前已有 156 条纯逻辑契约测试并接入 release verify，渲染/焦点/落盘等交互验收仍待人工。〔2026-10-20：2026-10-19 又补多批纯逻辑/契约测试（工作流进度、镜像、审批、输入、diff 等）并接入 release verify；33 个场景的渲染与交互验收仍需人工。〕

**阶段验收：** 每个域有对照旧版的 E2E 清单且全绿；插件负载与 Host sidecar 的断网语义符合预期（Host 随包，核心不依赖网络；无缓存的插件功能提示"需下载组件"）；三平台安装包体积按 DR-M5-2b 的两份预算持续测量。〔2026-10-17 DR-M5-2b：体积不再作为阻断门禁；原单一「核心 ≤30MB」目标保留为历史口径。〕

---

## M4 轻更快性能轴 + 自研验收与 UI 收口（2026-10-23 重排；M4a 1–2 周 → M4b 2–4 周）

> **2026-10-23 重排**：Rust 改造不能只看体积，还要在性能与效率上提高。本节把「轻更快」从口号
> 变成三类可测指标，并把删除/重做的顺序排进 M4a/M4b 与 M5a/M5b；原 M4 的自研验收与 UI
> clean-room 替换作为**并行子线**照常推进，不与性能轴互相阻塞。
>
> 三条不变的约束：① 删除 TS 运行时前它仍是 oracle（DR-07）；② 性能目标必须标注测量面
> （DR-T5-1 的口径扩展）；③ 所有删除项都要过 clean-room（黑盒对拍，不读上游衍生实现）。

**目标：** 删光上游衍生代码（并行子线）；同时给出体积/冷启动/常驻的实测基线，并把 Host 并入壳。

### 基线（2026-10-23 实测）

**发运集组成**（`dist-host-sidecar/ship/`，未压缩字节；门禁口径是安装包字节，沿用 DR-T5-1）：

| 组成 | 体积 | 说明 |
| --- | --- | --- |
| `node/` | **89 MB** | 随包 Node 运行时；最大单项 |
| `host-sidecar/` | 35 MB | Host bundle + external 依赖闭包 |
| `agent/glm/` | 20 MB | `mode.cjs` + 官方插件 + bundled-skills |
| `tools/` | 8.6 MB | cua-helper / browser-import / ripgrep |
| `config/` | 0.2 MB | 内置 provider 配置 |
| **合计** | **≈153 MB** | 未压缩载荷；压缩后另测 |

**进程与 IPC**：常驻 3–4 进程（壳 / sidecar Node / agent Node `mode.cjs` / CUA helper）；
帧协议是**二进制 `@mode/rpc` 序列化 + 4 字节大端长度前缀**（`stdioFrames.ts`），
每帧一次 serialize + 一次 deserialize + 拷贝，是流式路径的固定开销。
〔2026-10-23 更正：本节初版写的「JSON + base64」与现役实现不符——stdio 帧路径无 base64
编解码；实测与证据见 `apps/desktop-tauri/bench/ipc-throughput.ts` 与 §基线·IPC。〕

**IPC 吞吐（2026-10-23 基线，`pnpm --filter @mode/desktop-tauri measure:ipc-throughput`）**：
Node 侧帧编解码往返（serialize → 长度前缀 → 粘包还原 → deserialize，1 KiB 碎片投喂）：

| 档 | 帧均 | 编码 | 编码+解码往返 | 每帧堆增量（估算） |
| --- | --- | --- | --- | --- |
| control（小控制帧） | 26 B | 6.2 MB/s（25.1 万帧/s） | 3.3 MB/s（13.3 万帧/s） | ≈7.9 KiB |
| delta（流式文本增量） | 327 B | 43.1 MB/s（13.8 万帧/s） | 21.3 MB/s（6.8 万帧/s） | ≈10.6 KiB |
| bulk（256 KiB 附件分片） | 256 KiB | 1267.8 MB/s | 43.6 MB/s（174 帧/s） | ≈0.5 KiB |

读法：流式 delta 的 Node 侧帧成本约 **15 µs/帧**（往返 6.8 万帧/s）——按 30 fps 输出折算
占用 <0.1% 单核，**帧编解码不是流式延迟瓶颈**；端到端 p95 仍需 `bench/stream-latency`
覆盖（含壳与 WebView 段，未测）。bulk 档往返被 1 KiB 碎片投喂的 concat 拷贝主导，
只反映「分片到达」形态，不代表大帧整段吞吐。

**冷启动**：`T_ready` 825 ms / `T_surface` 962 ms（release，N=7，口径见 `tauri-renderer-host.md` §13.5）。

**代码量**：TS ≈394k 行（mode-cli 208k / services 109k / shared 44k / ui 19k / desktop 13k /
server 12k / rpc 3.5k）vs Rust ≈23.5k 行（mode-cua-ax 7.9k / agent-tools 3.8k / agent-core 2.1k /
sidecar-host 2k / platform 1.3k …）。

### 目标指标（每个数字标注测量面，混用即无效）

| 维度 | 指标 | 现状 | 目标 | 测量面 |
| --- | --- | --- | --- | --- |
| 轻 | 发运集核心载荷 | 59.77 MiB（2026-10-23 `measure:ship-core-payload` 实测：host-sidecar 31.60 + agent 19.49 + tools 8.50 + config 0.19） | **≤ 25 MB** | ship 集未压缩目录字节（node/ 除外；CI 记录不阻断） |
| 轻 | Node 运行时（核心发运集内） | 89 MB | **0**；npm-MCP 回退时按需出现 | 发运集目录 |
| 快 | 冷启动 `T_ready` | 825 ms | **≤ 400 ms** | `measure:cold-start` 同口径 |
| 快 | 首屏 `T_surface` | 962 ms | **≤ 600 ms** | 同上 |
| 快 | 流式增量端到端 p95 | 未测 | **≤ 80 ms**（本机 loopback） | 新增 bench |
| 省 | 常驻进程数 | 3–4 | **2**（壳 + helper） | 进程快照 |
| 省 | 会话空闲内存 | 未测 | **≤ 150 MB**（壳含 renderer） | 新增 bench |

> 目标值是承诺值；第一轮 bench 落地后若与现状冲突，**改目标而不是改尺子**。

### 可删除清单（三档）

**3.1 立即删**：只余旧 Electron 线删除（已完成，`d71ed992`）。
**裁决（2026-10-23）：SSH/WSL/Docker 后端保留**（含 `ssh2` 与 `/ws/remote/:id` 桥）——
Tauri Host 的 `defaultConnectStdio` 直接 `import("@mode/server/remote")` 建连（LR-4 第三阶段），
删掉会断掉这条产品能力；体积收益改由 M4b/M5a 承担。

**3.2 随 M4b「Host 并入壳」消失**：35 MB `host-sidecar/`；stdio 帧编解码与
`stdioFrames`/`stdoutGuard`（≈2k 行）；`bundledAgentRuntime.ts`（≈80 行）。

**3.3 随 M5a「Agent 切 Rust」消失（最大一块）**：Node 89 MB；`agent/glm/mode.cjs` 20 MB；
`packages/services` 运行时面 ≈43k 行（mode-agent / session / model-provider / process /
skills / subagents / plugin-sync / mcp-*）；`apps/mode-cli` 运行时装配 ≈190k 行
（bootstrap / adapters / core / contracts / cli / dynamic-workflow，**TUI 14k 不在此列**）；
`packages/rpc` 3.5k；8 个 Node 依赖（node-forge / ws / axios / undici / node-pty / yaml /
smol-toml / yazl）。

**3.4 明确不可删**：`packages/ui` 19k + HTML/CSS；TUI 14k（DR-TUI-1，除非 CLI 要单二进制）；
IM bot SDK（**2026-10-23 裁决为可选 feature**：按通道动态 import、移出核心闭包，
larksuite 25 MB / wecom 568 KB）；插件/技能的 JS 执行面（执行面保留，承载方式改为
子进程 + 自家二进制，见 M5 P2 裁决）。

**删除的硬前置**：TS 运行时仍是 oracle（DR-07）——对拍矩阵全绿 + 真实模型 20 个端到端任务
验收 + 性能 bench 不回退，三者缺一不可。

### M4a（1–2 周）：bench 与基线

bench 脚本与基线入库（见「度量与预算」的性能四件）；发布门禁补「发运集核心载荷 ≤ 25 MB」的
**观测**项。**本阶段不做删除**。**可选项**：IM bot SDK 可选化（larksuite/wecom SDK 改按通道
动态 import，移出核心发运集闭包）。
判据：bench 全绿并写入度量表；三类指标基线落库；若做可选项：未启用 bot 通道时启动不受影响、
SDK 不出现在核心闭包清单。

〔2026-10-23 进度：ipc-throughput 基线已入库（§基线·IPC，含「JSON+base64」描述更正）；
memory 脚本已入库并用现役实例验证分类器，30 min 正式基线待无实例窗口；cold-start 的
`T_agent_ready` 第三段经调研**撤销**（agent 懒启动，冷启动无此事件，改挂首任务延迟，见
「度量与预算」）；**「核心载荷 ≤25MB 观测项」已接线**——`measure:ship-core-payload`
量 ship 集四目录未压缩字节和（node/ 除外），release.yml 三平台腿记录不阻断，本机实测
**59.77 MiB**（host-sidecar 31.60 + agent 19.49 + tools 8.50 + config 0.19，over-target
标注正常；与 2026-10-23 基线 ≈63MB 同量级）；stream-latency 的 **agent 段**基线已入库
（中位 3ms / p95 35ms，详见「度量与预算」），Host→renderer 段待产品壳 E2E；
性能四件的脚本面至此全部落地，memory 的 30min 正式基线待无实例窗口。
**可选项 IM bot SDK 可选化：动态 import 半件已落地**（2026-10-24）——wecom 的静态
值 import 收口为 `wecomSdk.ts` 懒加载器（feishu 侧此前已是动态），Host 启动不再拉起
两个 SDK 的模块图（bundle 自验通过 + 守卫测试钉住「无静态值 import」）；
**「移出 ship 闭包」半件未做**：SDK 仍在 `RUNTIME_EXTERNALS`，直接摘除会让 esbuild
把 25 MB larksuite 内联进单文件 bundle（更糟）或让已装产品的企微/飞书通道
module-not-found；真正摘除需要「按需资产下发」基建（复用 AssetCache 骨架）或
产品拍板「无 SDK 时通道 fail-closed」，属待人工/后续项。〕

### M4b（2–4 周）：Host 并入壳 + IPC 换代

先把 `settings / credentials / task-index sqlite / file ops` 四件最小面 Rust 化
（`node-forge`→ring、`node:sqlite`→rusqlite），再把 `host-standalone` 的服务装配搬进壳的
async runtime（tokio + tauri state），renderer↔Host 从「stdio 帧 + 二进制 RPC 序列化」
改为同进程通道 / CBOR〔2026-10-23 更正：原写「JSON+base64」与现役实现不符，见 §基线·IPC〕。
收益：35 MB 消失、`T_ready` 保守省 200–400 ms。
判据：同口径复测 `T_ready` ≤ 500 ms；35 MB 载荷从发运集消失。
风险：服务面依赖 Node 生态，四件最小面不先行会陷入「为并入壳先重写一半服务」。

### 并行子线（M4a/M4b 期间照常推进，不阻塞性能轴）

**Files:**

- Create: `packages/ui-next/`（或按 `DESIGN.md` 重建 `packages/ui` 替代品）、`scripts/upstream-overlap-check.ts`（对 `upstream/zcode-baseline` 逐文件哈希对比 + 品牌归一哈希对比，口径见 `docs/specs/upstream-overlap-metric.md`：逐字节一致 / 仅品牌归一一致 / 实质不同三档。**2026-10-18 已交付 report 模式**；文件名用 `.ts` 以过语言政策）
- Modify: `THIRD-PARTY-NOTICES.md`、`third-party/copied-components.json`（重建：删除全部"上游衍生"条目，保留第三方组件条目）、`NOTICE.md` §4（diff 清零后改写为"当前代码不含上游实现，历史版本的 Apache 义务见基线分支"——**仍保留 zai-org/ZCode 链接与历史声明，不删**）


**子计划（2026-10-18）：** `docs/plans/2026-10-18-m4-overlap-reduction.md`——首轮 report 的
模块残余清单（ui 3.3 万行、services 1.3 万、desktop 9 千…）、P0–P4 阶段划分，以及
「Host sidecar / 传输层算不算自研面」的**待裁决口径**（选 (b) 必须改路线图与 spec，
不能在脚本里静默排除）。UI 替换的开工前盘点见
`docs/plans/2026-10-18-m4-ui-replacement.md`（`packages/ui/src` 32,626 行的子目录拆分、
P1.1–P1.7 顺序、验收与两项待裁决）。


**Interfaces:**

- Consumes: M1–M3 全部产出；`DESIGN.md`（作为 UI 重写的唯一规格）。
- Produces: `pnpm verify:self-developed`（= upstream-overlap-check + scancode/licensee 扫描 + 台账新鲜度），进 CI 必跑。


- [ ] **UI 替换**：按 `DESIGN.md` + 既有交互 spec 重写组件（渲染的是新 Tauri 前端，不回填旧 renderer）；逐屏与旧版截图对拍（允许视觉差异、不允许功能差异）。〔2026-10-18 盘点完成：`packages/ui/src` 重叠 32,626 行（lib 5,778 / components 4,703 / settings 3,402 / v4 2,979 / hooks 2,825 …）；**2026-10-19 D8 裁决：原地重建 `packages/ui` + 逐屏 adapter，store/hooks 不动，旧实现验收后删除**；P1.1–P1.7 静态结构与接线映射见 `2026-10-18-m4-ui-replacement.md` / `2026-10-19-m4-ui-adapter-map.md`；**2026-10-19 清单内 21 个基础件模块已全部切到 adapter**（button 226、toast 63、dropdown-menu 38、dialog 33、select 28、switch 24、tooltip 18、popover 16、badge 15、tabs 12 等），旧 `components/ui/*` 保留到人工看屏验收后删除；后续波次的 `collapsible`(14)/`spinner`(11)/`context-menu`(14)/`command`(7)/`label`+`kbd`/`hover-card`(3)/`input-group`+`scroll-fade`/`chart`(6)/`resizable`(3)/`diff-viewer`(3)/`lightweight`+`highlighted` diff(2+2)/`flip-metric-value`(1)/`pdf-viewer`(6)/`pptx-selection-action-bar`(1)/`code-viewer` 全部渲染点/`pptx-preview-viewer` 亦已切换；**业务代码对旧 `components/ui/*` 的引用已清零**，旧文件待人工看屏后删除（清单见 `docs/plans/2026-10-19-m4-ui-acceptance-checklist.md`）〕
      〔2026-10-20 回填：0.0.61 真机验收发现并修复 Dialog 遮罩未 portal、二级菜单不互斥、
      机器人页控制列溢出、Web/手机远控入口缺失与远端模型加载态误导；业务代码对旧
      `components/ui/*` 的引用保持为零，旧文件继续等待看屏验收后删除；UI foundation
      adapter 波次仍在继续。〕
- [ ] **重合度归零**：`upstream-overlap-check` 在 CI 断言"逐字节一致 + 仅归一一致"两档的行数占比 = 0%；首次运行产出的残余清单按模块排期清零。〔2026-10-18 工具已交付 report 模式；**裁决 (c)**：`packages/services|shared|rpc|server|provider` 为 M4 范围豁免、随 Rust 运行时迁移清零，单列 deferred（514 文件/121,181 行）；M4 计分面 headline **9.20%**（2026-10-21 复测：identical 2,613 文件/21,846 行，normalized 145 文件/15,395 行，共 404,926 行；分母从 462,066 降至 404,926 含旧 Electron 线删除的影响，不能全部归因于重写；2026-10-24 复测 **9.17%**（identical 21,846 行不变、normalized 15,367 行 −28、分母 405,796 行 +870——本周新增 bench/协议修复等自研代码把分母做大，非重写面变化）；2026-10-20 为 9.80%，2026-10-19 为 9.83%–9.84%，2026-10-18 为 10.60%）；`third-party/**`、`.agents/skills/**`、`scripts/license-texts/**` 单列不参与；`pnpm verify:self-developed` 为 strict，CI 清零前只跑 report；模块清单与阶段见 `2026-10-18-m4-overlap-reduction.md`，口径见 spec `upstream-overlap-metric.md`〕
- [ ] **第三方台账重建**：新依赖全量盘点（`scripts/licenses.mjs` 口径延续）；`copied-components.json` 中上游衍生条目删除、第三方条目（shadcn 思路自写组件、Material 图标去留、插件图标 reviewRequired）逐条复核。〔2026-10-18 缺口盘点见 `2026-10-18-m4-third-party-ledger.md`：Rust 台账只覆盖 CUA 的 63 个 crate，M2/M3 运行时依赖图 170 个中 142 个未入账、Tauri 壳 560 个中 485 个未入账；**T2 覆盖门禁已实现**（`pnpm check:rust-ledger`，实测 union external 558 / covered 44 / missing 514，report 模式）；**T1 生成器已实现**（候选台账 577 组件 / 300 份许可证文本；47 条按声明标识自动附标准文本，其中 **3 条仍需人工判断**且文本已附：`selectors` MPL-2.0、`r-efi` ×2 含 LGPL 选项；输出在 `.plan` 未入库，待 T3 Linux CI 重生成与人工 review；见 `2026-10-18-m4-rust-ledger-review.md`）；本机 `licenses.mjs` 因 EMFILE 无法重生成，权威路径是 Linux CI（需人推送）〕〔2026-10-20：`pnpm check:rust-ledger` 仍为 report——union external 558 / covered 44 / missing 514，覆盖门禁未转 strict。〕
- [ ] **法务收口**：贡献者协议（CLA/DCO）挂进 PR 流程；软著登记材料（源码前后 30 页 + 说明）；"Mode"图形组合商标申请材料；`docs/specs/p2-mode-naming.md` 必须保留清单逐条复核不动。

**阶段验收：**（性能轴）`T_ready` ≤ 500 ms、35 MB 载荷从发运集消失、bench 四件全部有基线入库。（子线）`verify:self-developed` 全绿（diff 0%）；UI 全量替换完成、旧 renderer 代码从产物中消失；第三方台账无 reviewRequired 中"上游衍生"类遗留；NOTICE §4 新措辞经人工复核。

---
## M5 Agent 切换与发布收口（2026-10-23 重排；M5a 4–8 周 → M5b 收口）

**目标：** Agent 运行时切到 Rust 并让 Node 从**核心**发运集退场；新线 GA、旧线 EOL、四仪表门禁落到终值。

### 需要重做的模块（2026-10-23 排序）

- **P0-B Agent 切产品路径**：Host 侧新增 `agent_runtime: rust | node` 双跑开关（默认 node，
  验收后翻转）；Rust 路径进程内调用（不经 stdio）；parity matrix 从「两侧夹具对拍」升级为
  「同一批夹具在两种运行时下跑」。收益：Node 89 MB → 0、agent 20 MB → 0、进程 4 → 2、
  流式事件少一层 JS 分配。**风险**：`agent-tools` 目前只有 Read/Write/Edit/Bash，
  **必须先补齐工具注册表与权限等级**，否则 parity 会红。
- **P1-A provider / 凭据 / 设置 / 任务索引**：provider 5.5k TS + shared 44k 协议面 → Rust
  （reqwest + rustls 已在 `crates/platform` 用过）；`node-forge` → ring；`node:sqlite` → rusqlite；
  shared 拆成「serde 真源 + TS 镜像」，减少双写。
- **P1-B CUA helper 合并入壳**：再省一个进程与一份 IPC。**风险**：独立进程是有意的安全隔离
  （`lan-remote-computer-use` spec），无安全评审不动手；先量化它的内存与启动成本再决定。
- **P2 插件宿主（2026-10-23 已裁决：A + 按需回退）**：维持 bundle-only 契约与独立子进程
  （DR-PH-1），子进程可执行文件从「随包 Node」换成「带嵌入 JS 引擎（QuickJS 级）的自家二进制」；
  协议（DR-PH-2 NDJSON JSON-RPC）与资产层（`crates/plugin-host` 的 manifest/AssetCache）不变。
  稳定性：子进程隔离保留（不做进程内嵌入）、无 native ABI/DLL、bundle 是冻结字节。
  通用性：作者契约零变化；native addon 类插件 fail closed；npm-MCP 的通用性由「按需 JS 运行时
  资产」回退承担（sha256 + CDN，复用 AssetCache 骨架）——89 MB 的 Node 从「随包常驻」变成
  「装了 npm MCP 才出现」。其余两项（IM bot Rust 化、TUI ratatui）仍按需评估。

### M5a（4–8 周）：Agent 切产品路径 + Node 退场

Agent 工具注册表补全 → 双跑开关 → parity 升级 → 翻转默认值 → 删除 §「可删除清单 3.3」全部内容。
**硬前置**：插件宿主 A 形态（或 npm-MCP 按需 JS 运行时回退）就绪，否则删掉 Node 会让插件与
npm MCP 全部失效。
判据：对拍矩阵全绿；真实模型 20 个端到端任务验收；`T_ready` ≤ 400 ms、`node/` 从**核心**
发运集消失；插件与 npm MCP 在无随包 Node 下可用。

### M5b：发布与收口

- [ ] **发布工程**：Tauri updater + 签名（三平台）、`release.yml` 只出 Tauri 产物、块映射/差分更新验证。〔2026-10-19：Windows Tauri 自动发版已连续跑通（v0.0.57 / v0.0.58：push → verify → 五腿构建 → publish）；2026-10-20 回填：v0.0.59 的 CI 首次三平台腿全绿，`248b0937`（v0.0.60）起三平台安装包与各自 `sha256` 一起发布，v0.0.60 / v0.0.61 已按该口径发布；0.0.61 真机核心链路通过。2026-10-21 更新：`release.yml` 已只出 Tauri 产物（`d71ed992` 删除 `desktop` job）；三平台签名与差分更新仍未完成，macOS/Linux 真机安装/启动/升级仍属残余风险。〕
- [ ] **旧线 EOL**：Electron 线打最后版本公告 EOL；`docs/plans` 与 spec 标注归档；仓库语言终态 = Rust + TS + HTML/CSS（目标 ≥99% 字节来自这两门 + 平台文件）。〔**2026-10-20 用户决定取消旧 Electron 线**：新增 `docs/specs/legacy-electron-eol.md`，删除 Electron main/preload/scheduler、Electron host 入口、electron-builder/electron-updater 打包与 `release.yml` 的 `desktop` job；保留 Tauri 复用的 renderer、host/host-standalone、agent/CUA 产物与 fnOS job。**代码与 CI 收口已完成并提交**（`d71ed992`，326 文件）：
typecheck / lint / architecture / language policy / desktop tests / desktop-tauri tests /
`build:package-inputs` 均通过；旧线回滚路径已删除。2026-10-21 补充：语言政策允许清单同步删除 34 条旧线陈旧条目（清单 162→128 条，陈旧 0），`check-language-policy` 通过。剩余 EOL 公告与历史文档归档。详见该 spec。〕
- [ ] **数据迁移终验**：`.mode` 数据根原样复用（协议未变）；从旧线最后一版升级到新线的实机迁移演练。〔2026-10-20：0.0.61 与旧 Electron 0.0.52 已在同一台机器并存安装，新线核心任务链路可用；正式的「旧线最后一版 → 新线」升级演练仍未执行。〕
- [ ] **三指标仪表入库**：`pnpm verify:self-developed`（重合度 0）、`scripts/check-language-policy.mjs`（语言）、CI 记录安装包体积（按 DR-M5-2b 的两份预算：核心 ≤30MB；Node 运行时 + sidecar 单独测量）。〔2026-10-18：语言门禁已在 `verify`；重合度工具已交付但只跑 report（M4 计分面 headline 10.60%，deferred 另计；M4 清零后改 strict）；体积按 DR-M5-2b 为持续测量项〕〔2026-10-20：语言门禁通过；重合度 report 降至 9.80%；0.0.62 本机 gnu 构建整包 51.79 MiB，三项均未成为阻断门禁。〕〔2026-10-21：重合度 report 9.20%（37,241 / 404,926 行）；语言门禁通过且允许清单陈旧条目清零；三项仍未成为阻断门禁。〕〔2026-10-24：重合度复测 9.17%（37,213 / 405,796 行，分母增长来自本周自研代码）；Rust 台账 `check:rust-ledger` 复跑仍为 union external 558 / covered 44 / missing 514，与登记一致、无新增入账缺口；三项仍未成为阻断门禁。]
- [ ] **Agent 真源翻转**

- [ ] **Agent 真源翻转**：删除 TS 运行时前把协议真源从 TS 翻到 Rust（DR-07 的翻转点写进
      `agent-runtime-parity-matrix.md`），parity matrix 从「两侧夹具对拍」改为「单真源 + 回归」。
- [ ] **三仪表 → 四仪表**：在原三件（语言/重合度/体积）上加**性能门禁**（`T_ready` 与核心载荷），
      与安装包字节同口径标注测量面。

**阶段验收：** 三平台 GA 发布；四仪表全绿并进必跑门禁；插件与 npm MCP 在无随包 Node 下可用；
20 个真实模型端到端任务验收通过。

---
## 风险与对冲

| 风险                                          | 对冲                                                                                                  |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| 行为对拍测不出隐性语义（双链路、owner/lease） | M2 对拍矩阵三层（工具序列/文件快照/事件流）+ 20 个真实任务盲跑；每改 stream/snapshot/queue 双链路双跑 |
| 插件/浏览器自动化等 TS 生态无法 Rust 化       | DR-01 sidecar 边界把它们挡在核心外；浏览器自动化（playwright-core 仅 TS）明确归 sidecar               |
| clean-room 被 AI 提示词无意破坏               | 禁读规则写进 AGENTS.md 与任务模板；PR 检查清单 + M4 硬门禁兜底                                        |
| 工期失控（单人 18–30 个月）                   | 阶段门禁可停可续：每个 M 结束都是可发布检查点；旧线持续维护保住产品不失血                             |
| 并行会话/多人同仓冲突                         | pathspec 提交纪律（已入记忆）、新目录（`crates/`、`apps/desktop-tauri`）与旧栈零重叠                  |
| 删除 TS 运行时过早，对拍失去 oracle | 双跑开关 + parity matrix CI 门禁 + 20 个真实任务；翻转点写进 `agent-runtime-parity-matrix.md` |
| 插件宿主 A 未就绪就删 Node | M5a 硬前置：A 形态或 npm-MCP 按需 JS 运行时回退先就绪；AssetCache 校验 sha256 |
| 性能目标与实测冲突 | 目标值是承诺值；基线落库后**改目标不改尺子**（DR-T5-1 口径扩展） |

## 度量与预算

- **仪表三件**：语言占比（linguist + language-policy 脚本）、上游重合度（upstream-overlap-check）、安装包字节（CI 记录）。
- **性能仪表四件（2026-10-23 新增；同日两处口径落地/更正）**：
  - `bench/cold-start`（`T_ready` / `T_surface` 两段，既有 `measure:cold-start`）。
    **口径更正（2026-10-23）**：初版写的第三段 `T_agent_ready` **撤销**——agent 是懒启动的
    （`modeAgentProcessManager.markReady` 只在首次 provider/model 门禁通过后调用；启动即零
    Helper 常驻是刻意设计），冷启动路径上不存在 agent ready 事件，硬加探针就是改被测对象。
    agent 启动耗时改挂**首任务延迟**（spawn → ready 的 `startupDurationMs`，已有
    `onReady` 结构化上报），与 `bench/stream-latency` 同属首任务面，不混入冷启动口径。
  - `bench/stream-latency`（`text_delta` 产生到 renderer 收到的 p50/p95）——**agent 段基线
    已入库**（2026-10-23，`measure:stream-latency`）：真 CLI（oracle driver + 本地 mock
    provider，零计费）测「模型端点 SSE 写出 → agent stdout text_delta 帧」段，n=10
    **中位 3 ms / p95 35 ms**（首轮 35ms 为 JIT 预热，稳态 3–4ms）。**不含** Host 转发与
    WebView 段——对照「端到端 p95 ≤ 80 ms」这是下界组成段；Host→renderer 段需产品壳
    E2E，待做。mock/driver 各加一个纯观测钩子（`onContentBlockDeltaWrite`），oracle
    既有回归全绿。**Host 段基线已入库**（2026-10-24，`measure:stream-latency-host`）：
    真 Host spawn 真 CLI + 本地 mock provider，经 mode-task 频道测「mock SSE 写出 →
    频道订阅者收到 agent_message_chunk」，5 turns：冷 turn 39 ms（含 agent spawn），
    稳态 **中位 4 ms / p95 5 ms（n=4）**。前一日的「agent 第 2 代 stdout_closed」阻塞
    已解：stderr wrapper 抓到 CLI 真因——数据根无归属文件 → CLI 判 unowned →
    rename「备份让路」撞 Host 目录句柄 EPERM；修法是起 Host 前先
    `initializeDataRootNonInteractive`。八条接线坑与 wrapper 诊断法见脚本文件头。
    **顺带发现（同日已修，两半边均收口）**：CLI 的 `turn.started` payload 带 `executionStartedAt`、
    `tool.updated`/`started` 带 `readOnly`+`sideEffectScope`，Host strict schema 曾因未声明
    unrecognized_keys 整事件丢弃——两处均已补声明，`KNOWN_V1_PAYLOAD_LAG` 清空为墓碑绊网；
    端到端证据：bench 丢弃日志清零、协议夹具 started 帧全量 parse 通过
    （详见 `tauri-agent-runtime.md` §6.6）。
    两段相加 ≈ 8–9 ms（稳态），对照「端到端 p95 ≤ 80 ms」余量巨大；WebView 段仍待产品壳 E2E。
  - `bench/memory`（壳 / sidecar / agent 三进程 30 分钟 RSS 曲线）——**脚本已入库**
    （`measure:memory`，2026-10-23）：Win32_Process 过滤采样 + 安装目录/命令行特征归类，
    分类器已对现役实例验证（shell/sidecar/agent 全命中）；**前置**是被测机无运行中实例
    （单实例守卫会让新壳静默秒退，脚本显式检查并报错）。90 s 冒烟被「用户实例在跑」阻塞
    （不代杀），30 min 正式基线待无实例窗口跑并抄数。
  - `bench/ipc-throughput`（帧吞吐 MB/s 与每帧分配次数）——**基线已入库**（2026-10-23，
    数字见 §基线·IPC；同时更正了「JSON + base64」的过时描述）。
  落点 `apps/desktop-tauri/bench/` 与 `pnpm measure:*`；**改任何性能代码前先入库基线**。
- **目标值**（承诺值，标注测量面）：核心发运集载荷 ≤25 MB、核心发运集内 Node =0、`T_ready` ≤400 ms、`T_surface` ≤600 ms、流式增量 p95 ≤80 ms、常驻进程 2、空闲 RSS ≤150 MB。冲突时改目标不改尺子。
- **预算**（沿用 2026-10-09 估算）：DeepSeek API 全程 ¥5k–3 万（flash 为主）；人力为大头：2–3 人 × 12–17 个月 ≈ ¥100–200 万，单人则为时间机会成本。
- **子计划节奏**：每阶段开工前 1 周内用 `writing-plans` 产出该阶段细粒度计划并过评审；本文件只在阶段边界变化时修订。
