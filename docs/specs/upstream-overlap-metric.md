# Spec：上游重合度指标（M4 / M5 仪表）

状态：2026-10-18 首版。对应路线图 M4「重合度归零」与 M5「三指标仪表入库」第 2 项。
工具：`scripts/upstream-overlap-check.ts`（语言政策要求 `.ts`；路线图原文写的 `.mjs` 不再适用）。

## 目的

给「与上游 `upstream/zcode-baseline` 的重合度」一个**可复现、可解释**的读数，供 M4 逐模块清零、
M5 作为硬门禁。它只产出**统计与路径清单**，绝不输出上游文件内容（clean-room 的提示词纪律）。

## 输入

| 输入 | 说明 |
| --- | --- |
| 基线 ref | 默认 `upstream/zcode-baseline`；可用 `--baseline <ref>` 覆盖 |
| 当前树 | `HEAD`（不是工作区）：指标衡量的是已提交状态 |
| 文件清单 | 两侧 `git ls-tree -r -z`，只比较**同一路径**的文件 |

基线不存在时：report 模式打印 `baseline unavailable` 并退出 0（CI 克隆通常没有这个本地分支）；
`--strict` 退出 2（无法验证，不假装通过）。

## 三档口径

| 档 | 判据 | 计入分子 |
| --- | --- | --- |
| `identical` | 同路径 + blob sha 相同（逐字节一致） | 是 |
| `normalized` | 同路径，blob 不同，但**品牌归一后**文本相同 | 是 |
| `distinct` | 同路径，其余情况 | 否 |
| `ours-only` | 当前树有、基线无 | 否（分母也不含） |
| `upstream-only` | 基线有、当前树无 | 否（仅报告） |

**品牌归一（N1）**：先 `\r\n → \n`（检出格式差异不是内容），再按下列顺序做**大小写不敏感**替换：

1. `zcodium` → `mode`
2. `z-code` → `mode`
3. `z_code` → `mode`
4. `zcode` → `mode`

只归一品牌词，不改空白、大小写、标点或标识符结构。归一后雷同但品牌无关的重复**不得**被
这一档吸收；它们应落 `distinct` 并在评审中单独处理。

二进制/非 UTF-8：不参与归一档（只有 `identical` 或 `distinct`），行数记 0。

**vendored 排除**：`third-party/**` 与 `.agents/skills/**` 属 clean-room 政策里的「不适用」面
（第三方复制件），**不参与 headline**，单独报告文件/行数与两档计数。把它们算进「自研重合度」
会让 0% 永远不可达，也会掩盖真正的自研面。

## 指标

- **文件计数**：各档文件数；`ours-only` / `upstream-only` 单列，不参与重合度（vendored 不计入）。
- **行数**：对两侧都是 UTF-8 文本的同路径**非 vendored** 文件，按 `\n` 切分（末尾换行不产生空行）。
  - 分子 = `identical` 与 `normalized` 的行数（取当前树侧）；
  - 分母 = 所有同路径**非 vendored** 文本文件的行数（当前树侧）；
  - **headline = 分子 / 分母**，即路线图所说「两档的行数占比」。
- 二进制文件只影响文件计数，不影响 headline。
- vendored 单独一组输出（`vendored (excluded from headline): …`），口径同三档，但不进 headline。

## 退出码

| 模式 | 行为 |
| --- | --- |
| 默认（report） | 打印摘要与按顶层目录的分布，退出 0 |
| `--json` | 输出机器可读 JSON（同字段），退出 0 |
| `--list` | 追加两档的文件路径（仅路径，无内容） |
| `--strict` | headline > 0 或基线缺失时退出 1/2；M4 完成前的 CI **不启用** |

## 与 CI 的关系

- `package.json`：`verify:self-developed` = `--strict`（终态门禁）；`check:upstream-overlap` = report。
- `release.yml` 的 verify 步骤：**当前只跑 report**，并在日志里登记 headline；M4 完成、指标归零后
  才改为 `pnpm verify:self-developed`。在归零之前把它接成阻断门禁会让每次发版必红，等于
  用一个已知不满足的阈值锁死发布线（与 release.yml 现有「wired but not enforced」的说明一致）。

## 验收

1. 对同一 ref 连跑两次输出逐字相同（无时间戳、无工作区依赖）。
2. `normalizeBrand` 的纯函数用例覆盖四种品牌写法与 CRLF；聚合用例覆盖三档与分母口径。
3. 默认模式在基线缺失时退出 0 并给出 `baseline unavailable`；`--strict` 退出 2。
4. 输出不含任何上游文件正文（只有路径、计数、哈希/比率）。
