# M4 上游重合度清零：模块残余清单与分阶段计划（2026-10-18）

> 输入：`scripts/upstream-overlap-check.ts` 的首轮 report 与 `--json` 文件清单
> （口径见 `docs/specs/upstream-overlap-metric.md`）。本文件只列**路径、行数与阶段**，
> 不包含上游实现内容。

## 当前读数（2026-10-18，baseline `upstream/zcode-baseline` @ `aac47556`）

```text
common files: 4687（ours-only 2896，upstream-only 11725）
identical: 2799 files / 41,066 lines
normalized:  295 files / 35,878 lines
distinct:   1593 files / 507,082 lines
headline:   13.17%（76,944 / 584,026 lines）
excluded:    455 files / 82,661 lines（third-party、.agents/skills、license-texts）
```

M4 验收目标是 `pnpm verify:self-developed`（strict）通过，即 **重叠行数 = 0**。
当前 13.17% 折合约 7.7 万行，集中在四个包。

## 残余分布（只含 identical + normalized，已排除豁免面）

| 模块 | 文件 | 行数 | identical | normalized |
| --- | --- | --- | --- | --- |
| `packages/ui` | 432 | **32,648** | 299 / 19,334 | 133 / 13,314 |
| `packages/services` | 99 | **12,766** | 35 / 4,095 | 64 / 8,671 |
| `packages/desktop` | 1,229 | **9,135** | 1,187 / 4,214 | 42 / 4,921 |
| `packages/shared` | 66 | **6,183** | 48 / 3,911 | 18 / 2,272 |
| `packages/rpc` | 20 | **3,923** | 18 / 3,235 | 2 / 688 |
| `packages/server` | 26 | 2,813 | 11 / 563 | 15 / 2,250 |
| `packages/provider` | 15 | 2,362 | 9 / 946 | 6 / 1,416 |
| `packages/formal-proof` | 7 | 1,896 | 5 / 968 | 2 / 928 |
| `scripts` | 18 | 1,883 | 17 / 1,423 | 1 / 460 |
| `packages/web` | 1,150 | 1,283 | 1,148 / 1,185 | 2 / 98 |
| 其余（model-option-map / provider-node / client / 根配置等） | 29 | 2,117 | — | — |

读数解释：

- `packages/ui` 是 M4 UI 重写的直接目标，**占比约 42%**；
- `packages/services` / `shared` / `rpc` / `server` / `provider` 是 Host sidecar 与传输层
  （M3 已决定复用），**不能靠「M4 替换 UI」自然清零**——这是本计划需要单独裁决的口径问题；
- `packages/desktop` 的 1,229 个文件只占 9,135 行（平均 7 行/文件），多数是 Electron main/host
  的小模块与 `dist-types`，随 M5「旧线 EOL」整目录删除即可归零；
- `packages/web` 的 1,150 个文件只有 1,283 行，绝大多数是 `public/**` 静态资产（图标等），
  需要先分类再决定删除/重画，不是重写代码。

复现：

```bash
pnpm check:upstream-overlap                      # 人读摘要
pnpm check:upstream-overlap -- --json > overlap.json   # 按模块排期
pnpm check:upstream-overlap -- --list            # 两档路径清单（无正文）
```

## 需要裁决的口径（阻塞 P3）

**问题：Host sidecar / 传输层算不算「自研面」？**

| 选项 | 含义 | 后果 |
| --- | --- | --- |
| **(a) 也清零** | `packages/services` / `shared` / `rpc` / `server` / `provider` 逐模块重写或删除 | 与路线图「重合度 = 0」字面一致；工作量最大，且要保证行为不变（services 的 60 个测试是现成规格） |
| **(b) 登记为「运行时基础设施」不计入** | 只对 UI / 新栈代码要求 0%，sidecar 内的旧实现保留 | 指标不再代表「仓库无上游实现」，与 clean-room 目的冲突，需要在 NOTICE/路线图明确改写目标 |

**本计划不自行选择。** 在裁决前，只推进与裁决无关的 P1/P2：它们要么本来就要删，
要么是 M4 明写的 UI 替换。选 (b) 需要同时修改路线图 M4/M5 的验收措辞，不能只改脚本。

## 分阶段计划

### P0：冻结新增重合（已就绪）

- CI 已有 report 步骤（`check:upstream-overlap`），每次发版记录 headline；
- 新代码评审时禁止「非接口约定所致的逐字雷同」（clean-room PR checklist）；
- 后续可加 per-PR delta：比较 base/head 的 `--json` 分子，**只允许下降**。先不接，避免在大规模
  删除（P1/P2）期间被路径移动误判。

### P1：`packages/ui` 替换（M4 核心，约 3.3 万行）

- 按 `DESIGN.md` + 交互 spec 重建组件；`packages/ui` 的上游衍生文件随替换删除；
- 验收：同一批屏的 E2E 场景通过（允许视觉差异、不允许功能差异）；
- 每完成一个目录，跑 `--json` 看该目录的 identical/normalized 是否归零。

### P2：Electron 线删除（随 M5 EOL，约 9 千行）

- `packages/desktop/src/{main,host,preload}/**`、`dist-types/**` 与 Electron 专属 web 资产随
  旧线 EOL 删除；Tauri 壳 + sidecar 已承接对应能力；
- 删除前先确认 M5「旧线 EOL」所需的最后版本与回滚分支。

### P3：运行时基础设施（待口径裁决）

- 若 (a)：按模块重写/删除，优先 `rpc` → `shared` → `provider` → `server` → `services`
  （依赖方向从底向上）；
- 若 (b)：修改路线图与 clean-room spec 的验收措辞，并在 `upstream-overlap-metric.md` 增加
  明确的前缀豁免（不能只靠脚本里的数组默默排除）。

### P4：docs / scripts / 静态资产

- `docs/**` 的上游文档：保留行为规格价值，重写为 Mode 口径或删除；
- `scripts/**`：Electron 线专用的构建/校验脚本随 P2 删除；通用工具按需重写；
- `packages/web/public/**` 资产：先按来源分类（Material 图标等第三方资产走
  `third-party/` 台账，而不是计为自研重合）。

## 验收与度量

1. `pnpm verify:self-developed`（strict）退出 0；
2. 每个阶段结束时 `--json` 的 headline 单调下降，阶段记录进本文件；
3. 删除类阶段需先跑对应包测试，确认没有把仍被新产品使用的行为删掉；
4. 法务措辞（NOTICE §4）在清零后由人复核，不在本计划内改写。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 为了指标好看而排除整个 `packages/**` | 口径变更必须改 spec + 路线图，且要有裁决记录；脚本不私自扩豁免 |
| 删除 Electron 代码时误删 sidecar 仍在用的契约 | 先跑 `packages/services` / `packages/shared` 测试与 Tauri 端到端，再删 |
| UI 替换期间路径移动导致 delta 抖动 | P0 的 per-PR delta 延后到 P1 结束再加 |
| 指标与 clean-room 目的脱节 | headline 只统计非豁免路径；豁免清单写明依据（第三方复制件/许可证原文） |
