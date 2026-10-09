# Computer Use 打包发布（Plan C）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让电脑控制进入**安装包与发版链**——helper 资产进 electron-builder、CI 构建 Rust addon、文案/合规同步、display 真黄金与验收清单落地；至此功能从「开发模式可用」变为「发布可用」。

**Architecture:** 不新增运行时行为（除声明的两处一行级修复）。四条线：(1) 打包：`pnpm build:cua-helper` 产物 stage 到 `packages/desktop/bundled-tools/win32-x64/cua-helper`，electron-builder `extraResources` 仿 ripgrep 模式入 `tools/cua-helper`（仅 win32）；(2) CI：release.yml 的 windows 构建腿装 rustup + `MODE_CUA_AX_TARGET=x86_64-pc-windows-msvc` 构建并 stage，cargo test/clippy 挂同腿（verify=ubuntu 跑不了 Win32 测试）；(3) 合规：NOTICE 双语行、mode-cua README/description、Rust crates 进三方台账（沿 `scripts/generate-third-party-notices.mjs` 的输入哈希机制）；(4) 收口：display 真黄金（core 自测，唯一能同时 import 两端的包）、实机验收清单、遗留一行修。

**Tech Stack:** 现有 node 脚本 + electron-builder 配置 + GitHub Actions + 既有三方台账生成器；Rust 工具链只在 CI windows 腿与本地（gnu 预案沿用 Plan A 终案）。

**Spec:** `docs/specs/computer-use-windows-runtime.md`（验收场景/风险/归属）；前序计划 Plan A（能力层）、Plan B（host runtime）均已完成并终审通过。

## Global Constraints

- 纯净室延续：不读 `C:\Users\Administrator\AppData\Local\Programs\ZCode`。
- **不改运行时行为**；允许的代码改动仅：Task 5 声明的两处（`node.ts` 注释、`index.d.ts` 声明）与打包/CI/文档脚本。
- **非目标（显式排除）**：`signal` 真中止接线；`pnpm fmt:check` 既有基线（~4338 文件）；mac helper（二期）；语言政策 allowlist（已在 `4ddddc3` 解决，本计划只验证为绿）。
- 并行会话共存：任何触碰 `third-party/`、`scripts/generate-third-party-notices.mjs`、`scripts/license-texts/`、`docs/specs/language-policy*` 的任务开局必须 `git status` 检查在途改动；**有在途改动 → 该子项停下报告（NEEDS_CONTEXT），不与之竞争编辑**。
- 提交只 `git add` 本任务文件；绝不 push。
- 验证命令沿用：`pnpm --filter @mode/cua test`、`CUA_INTEGRATION=1 …test:integration`、`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、`node scripts/check-language-policy.mjs`、`pnpm verify:pre-push`、cargo prelude（见 Task 2）。
- 已裁决沿用：`MODE_CUA_AX_TARGET` 默认 gnu、CI 注 msvc；语言政策 allowlist=spec 批准已完成；helper 零下载（不接 `__MODE_CUA_HELPER_BUILD_ID__`）。

## 文件地图

| 文件 | 动作 |
| --- | --- |
| `scripts/stage-cua-helper.mjs`（新）或既有 prepare 脚本扩展 | T1：`dist-cua-helper` → `packages/desktop/bundled-tools/win32-x64/cua-helper` |
| `packages/desktop/electron-builder.config.js` | T1：extraResources 增 win32 条目（仿 ripgrep） |
| `packages/desktop/scripts/ensure-local-runtime-assets.mjs` / dev 脚本 | T1：dev 流兜底（trace 后定） |
| `.github/workflows/release.yml` | T2：windows 腿 rustup+msvc+build+stage+cargo |
| `NOTICE.md` / `NOTICE.zh-CN.md` / `packages/mode-cua/README.md` / `package.json` description / 根 README 核对 | T3：占位→可用文案 |
| `third-party/*` + `scripts/generate-third-party-notices.mjs`（或手工节） | T3：Rust crates 台账（开局状态检查） |
| `apps/mode-cli/packages/core/test/cua-display-golden.test.mjs`（新）+ core `package.json` test 脚本 | T4：display 真黄金 |
| `docs/specs/computer-use-windows-runtime.md` 验收节 | T4：实机验收清单 |
| `packages/services/src/node.ts`（注释）、`packages/mode-cua/index.d.ts` | T5：一行修 |

---

### Task 1: 打包管线与 dev 流

**Files:**
- Create/Modify: stage 脚本（优先复用——trace `packages/desktop/scripts/ensure-local-runtime-assets.mjs` 与 `scripts/prepare-prebuilds.mjs` 谁拥有 `bundled-tools` 的生産语义，报告给出选择依据；新脚本则 `scripts/stage-cua-helper.mjs`）
- Modify: `packages/desktop/electron-builder.config.js`（`extraResources` 数组）
- Modify: 根 `package.json`（`build:cua-helper` 之后能一键 stage 的脚本串联，或 build 脚本内直接输出双落点——trace 后定）
- Test: `packages/desktop/tests/cua-helper-packaging.test.mjs`（新，沿 desktop tests `node:test` 风格：断言 extraResources 含 win32 cua-helper 条目且 filter 正确、stage 脚本幂等、manifest 三件齐）

**Interfaces:**
- Consumes: `pnpm build:cua-helper` 产物（`packages/mode-cua/dist-cua-helper/{entry.cjs,cua_ax.node,runtime-manifest.json}`，Plan A Task 9）；ripgrep 先例（`electron-builder.config.js` 内 `from: bundled-tools/${targetPlatform.key}/ripgrep, to: tools/ripgrep`，约 :632）；`resolvePackagedRuntime` 期望 `resources/tools/cua-helper`（Plan A 已测）。
- Produces: 打包态 `resources/tools/cua-helper` 三件（T5 全仓验证时以契约测试/手工解包断言）；dev 流一句话说明（写进 mode-cua README 的哪节由 T3 落，本任务在报告给出确切命令）。

- [x] **Step 1: trace**（报告贴出）：`ensure-local-runtime-assets.mjs` 全文（dev 缺 bundled-tools 时行为）、`prepare-prebuilds.mjs` 是否拥有 bundled-tools 生产、electron-builder `extraResources`/`filter`/平台条件写法（读 config 现有 win32-only 先例——如 tray icon 的条件表达式）、desktop `dev:desktop` 是否已串 prepare。
- [x] **Step 2: 失败测试先行**（desktop tests 风格：读 `electron-builder.config.js` 源或其导出对象断言条目；stage 幂等=跑两次结果一致+manifest sha 不变）
- [x] **Step 3: 实现**——stage 进 `bundled-tools/win32-x64/cua-helper/`（三件；`filter: ["**/*"]`）；extraResources win32-only 条件（**非 win32 构建不得引用该目录**——用 config 现有的平台条件机制，别发明）；dev 流：若 ensure-local 缺失则给出/接入「先 build:cua-helper+stage」的兜底（trace 结论决定改哪边）。
- [x] **Step 4: 验证**：新测试绿；`pnpm --filter @mode/desktop`（若有 test script）或 `node --test packages/desktop/tests/cua-helper-packaging.test.mjs`；`pnpm typecheck`、`pnpm lint`（desktop config 是 `.js`——已在 allowlist？核对，不在则属新增违规→**不加**（清单已定案），改用未被禁的扩展名或申请裁决——报告里说明）。
- [x] **Step 5: Commit** — `feat(desktop): cua-helper 打包 stage 与 extraResources 接线`

---

### Task 2: CI 接线（release.yml）

**Files:**
- Modify: `.github/workflows/release.yml`
- Test: 无新测试文件；以 YAML 解析 + 结构断言收尾（`node -e "…yaml 解析…"` 或 repo 若有 yaml 工具则用之；没有就用 python -c yaml）

**Interfaces:**
- Consumes: T1 的 stage 语义；`MODE_CUA_AX_TARGET` 裁决（默认 gnu，CI 注 **msvc**）；desktop job 矩阵（`runs-on: ${{ matrix.runner }}`，release.yml:257）。
- Produces: windows 构建腿在打包前具备 `bundled-tools/win32-x64/cua-helper`；cargo test + clippy 在 windows 腿执行。

- [x] **Step 1: trace**（报告贴出）：release.yml 全结构（release/verify/desktop/checksums/publish 各 job 的 steps、windows 腿的 step 顺序、缓存机制有无——rust 缓存挂哪）；verify(ubuntu) 现跑哪些测试。
- [x] **Step 2: 实现**：
  - windows 腿（`matrix.runner` 含 windows 的分支）在「desktop 打包步骤」之前插入：
    1. `dtolnay/rust-toolchain@stable`（或 actions/setup equivalents——**优先 repo 已有 action 惯例**，trace 后选；targets: `x86_64-pc-windows-msvc`）
    2. cargo 缓存（`Swatinem/rust-cache` 或 repo 既有缓存惯例）
    3. `MODE_CUA_AX_TARGET=x86_64-pc-windows-msvc pnpm build:cua-helper`
    4. `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` + `cargo clippy … -- -D warnings`
    5. stage（若 build 脚本未内联 stage，见 T1 结论）
  - **非 windows 腿**：必须跳过上述（helper 仅 win32 需要）；extraResources 条件已由 T1 保证。
  - 注意 `pnpm build:cua-helper` 在 msvc 目标下**不需要** llvm-mingw prelude（Build 用 CI 自带 MSVC；但注意脚本内 gnu-only 的 PATH/LIBNODE 拼接——T1/T2 联合验证脚本在无 llvm-mingw 的环境下 msvc 路径可用：本地无法模拟 CI，故在脚本里补一条「msvc 目标跳过 gnu prelude」的分支若尚不存在，报告指出改动归属（build 脚本=允许面）。
- [x] **Step 3: 本地可证的验证**：YAML 解析通过；用 `python -c "import yaml…"` 或 node yaml 库断言 windows 腿 steps 顺序（build 在打包前）；把「本地无法实证 CI 绿」如实写进报告（实证=推送后 Actions，由用户发起）。
- [x] **Step 4: Commit** — `ci(release): windows 腿构建并验证 cua-helper`

---

### Task 3: 文案与三方合规

**Files:**
- Modify: `NOTICE.md`, `NOTICE.zh-CN.md`, `packages/mode-cua/README.md`, `packages/mode-cua/package.json`（description）
- Modify（核对后按需）: 根 `README.md`/`README.zh-CN.md`（若有「不可用/占位」表述）
- Modify（**开局状态检查，见 Global Constraints**）: `third-party/*` + `scripts/generate-third-party-notices.mjs` + 生成产物

**Interfaces:**
- Consumes: NOTICE 现行 Computer Use 行（两语言各一行，搜索 `Computer Use`）；台账机制（`generate-third-party-notices.mjs` 输入哈希 + `third-party/inventory.json` + `licenses-notices.yml` CI）；Rust 依赖清单（`crates/mode-cua-ax/Cargo.toml`：napi, napi-derive, napi-build, windows, image, serde, serde_json + 各传递依赖——以生成器能收集的粒度为准）。
- Produces: 对外能力披露与实际一致；Rust 二进制内的三方许可证可追溯。

- [x] **Step 0（门）**: `git status --short -- third-party scripts/generate-third-party-notices.mjs scripts/license-texts docs/specs/language-policy*` —— **有在途改动 → 本子项 NEEDS_CONTEXT 停下报告（只做文案三件），不碰台账**。
- [x] **Step 1: 文案（无论如何都做）**：
  - NOTICE 双语行：把「不可用占位实现」改写为**保守准确**的能力披露——Windows：读取无障碍树/屏幕截图/注入键鼠/启动应用/锁屏探测；权限边界（无 TCC；UIPI 提权窗口拒绝=`action_unavailable`；锁屏 `permission_denied`）；mac 仍为占位（如实保留其行文若分平台）；引用不变的行文风格（表格式一句话+细节链接）。**逐语言核对两行语义一致。**
  - `packages/mode-cua/README.md`：占位 README → 真实描述（broker/runtime/helper 架构一句 + `pnpm build:cua-helper` + `MODE_CUA_DEV_ROOT` dev 用法 + `CUA_INTEGRATION=1` 测试入口）。
  - `package.json` description 同步（Plan A T1 deferred 项）。
  - 根 README grep `占位|placeholder|Computer Use|电脑控制` → 有则改，无则报告「无需改」。
- [x] **Step 2: 台账（门通过才做）**: trace 生成器如何接纳非 npm 源（`native-search/sources.json` 是先例）→ 以同模式加 `third-party/rust-sources.json`（crates 清单+license 文本）+ 生成器收集分支 + 跑生成器刷新 inventory/notices；跑 `licenses-notices.yml` 对应本地命令（trace workflow 里的 check 命令）确认绿。**若机制扩展 >~120 行或需要新依赖 → 停下报告最小方案再裁。**
- [x] **Step 3: 验证**: `pnpm typecheck`、`pnpm lint`、语言政策绿、（若动了台账）其 check 命令绿。
- [x] **Step 4: Commit** — 可拆两笔：`docs(notice): 电脑控制能力披露对齐实际` + `chore(third-party): Rust crates 台账接入生成器`（各自 pathspec）

---

### Task 4: display 真黄金 + 实机验收清单

**Files:**
- Create: `apps/mode-cli/packages/core/test/cua-display-golden.test.mjs`（落点规则见 Step 1）
- Modify: `apps/mode-cli/packages/core/package.json`（`test` 脚本——若该包无）
- Modify: `docs/specs/computer-use-windows-runtime.md`（验收场景节 → 可勾选的实机清单）

**Interfaces:**
- Consumes: core `createCuaToolResultDisplay`/`hasOfficialCuaFrameAuthority`（core 既有 build=`tsc`→dist，测试 import dist 或相对源？——core 是 TS：测试须 `node --test` + 先 build，或用 mode-cli 现有 TS 测试惯例（trace mode-cli 有无先例））；runtime 真实输出样例（可 import `@mode/cua`——core→mode-cua 已是既有依赖方向 ✓）；Plan B T7 替身测试（`packages/mode-cua/test/display-contract.test.mjs` 保留，作为包内契约锁）。
- Produces: 真消费方（core 本尊）跑通 display 黄金；验收清单入 spec。

- [x] **Step 1: 落点规则执行**（报告贴证据）：唯一合法落点=core 自测（core 同时依赖 mode-cua ✓、UI 不可被 services/core 依赖 ✓）。trace：mode-cli 是否已有 `node --test` on dist 的先例（`apps/mode-cli/packages/*/package.json` grep `"test"`）；verify CI 是否跑 mode-cli 包测试（release.yml verify steps）——**若无 CI 跑点，仍落地（本地可跑），报告注明 CI 接线归 T2 或后续**（若 T2 顺手可挂则挂，不强求）。
- [x] **Step 2: 失败测试**：canned runtime observe 帧对结果 + 动作收据 + 错误三形态 → `createCuaToolResultDisplay(..., true)` → `toolResultDisplaySchema.parse` + 断言 media dataUrl 被 `cuaScreenshotDetails` 正则识别（复制正则+来源行号）+ `hasOfficialCuaFrameAuthority` 真值 + errorCode/suggestedAction 透传。（比 Plan B 替身多的：真 `result-display.ts` 路径含 32KiB bound/truncated 分支——补一条截断用例，还 T7 欠账。）
- [x] **Step 3: 实现 + 绿**：core build 后 `node --test` 绿；mode-cua 替身仍绿（双锁）。
- [x] **Step 4: 实机验收清单**：把 spec 验收场景节改为可勾选清单（Plan A 验收清单 + Plan B 新增：透明启动、租约 busy、锁屏 permission_denied、frame ≤200KiB 卡片、插件开关关闭→明确不可用态、安装包内 `resources/tools/cua-helper` 三件存在——T5 实机步引用）。
- [x] **Step 5: Commit** — `test(core): display 真黄金 + spec 实机验收清单`

---

### Task 5: 收尾小修 + 全仓验证

**Files:**
- Modify: `packages/services/src/node.ts`（仅 :1246-1248 附近注释：删除/改写过期 `caller_timeout` 期望——Plan B T5 M1 出路；**注释级，不动逻辑**；改前重读上下文）
- Modify: `packages/mode-cua/index.d.ts`（补 `resolveElementIndex` 声明或在实现侧注释改「仅测试缝」二选一——读 Plan B 终审 Minor#9 的现场再定，报告写明选了哪条）
- Modify: `docs/superpowers/plans/2026-10-09-computer-use-packaging-release.md`（checkbox + 偏差附录）

- [x] **Step 1**: 两处小修 + 测试不回归（`pnpm --filter @mode/services test`、`pnpm --filter @mode/cua test`）。
- [x] **Step 2: 全仓验证（真实输出逐项贴）**: mode-cua 单测、`CUA_INTEGRATION=1` 集成、services、（T4 后）core 黄金、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、cargo test+clippy（prelude）、`node scripts/check-language-policy.mjs`（应绿）、`pnpm verify:pre-push`（应绿）、`pnpm fmt:check`（既有基线红——注明非本计划）。
- [x] **Step 3: 计划勾选 + 偏差附录**（本计划各任务实际偏差一行汇总）。
- [x] **Step 4: Commit** — `docs(cua): Plan C 收尾与 CI 黄金路点`（收尾实际提交信息，控制器定稿）

---

## 后续（不在本文件范围）

- mac 二期（Helper.app/TCC/签名）、`signal` 真中止、desktop 渲染层 E2E（若 T4 探测证明无 harness）、发版实机验证（安装包装机走 spec 验收清单——需用户发起推送后进行）。

## 任务实际偏差（Task 5 收尾回填）

- T1: 新测试/脚本用 `.mts`（语言政策门内解）；stage 内联进 build:cua-helper 串联；ensure-local 非致命指引。
- T2: windows-only 步骤为仓首用 `if: matrix.target == 'win'`；dtolnay/Swatinem 首用；CI 实证=推送后。
- T3: 生成器本机不可整跑（EMFILE+CRLF）→ readRustNotices 同源拼接、CI diff 门终裁（评审逐字节重建=会绿）；根 package.json 输入哈希代修（T1 前置）。
- T4: 黄金落 core（mode-cli 零 test 脚本/turbo 无 task）；`tsx --test` 先例；CI 路点移交本任务；截断语义按生产现实锁定（UI 三方消费方容错，评审核实）。
- T5: verify 并入「Shared, service, core and desktop tests」单步跑 core 黄金（build+test）；release.yml rust 两步补首用注释；`node.ts` 过期 `caller_timeout` 注释改写为现实行为（逻辑未动）；`resolveElementIndex` 选二选一之「保留不声明 + 实现侧注释明确仅测试缝」（index.js 注释改写，d.ts 不动）；全仓 10 项验证见 task-5-report.md。
