# 上游 ZCode 只读审计（2026-10-14）

**性质：只读审计。** 本文件与本次操作不改动 `main` 与工作区任何代码，也不新建/合并任何上游代码。
只读取了元数据（commit message、文件路径、blob 哈希），**未打开任何上游实现文件的内容**，
以遵守 `docs/specs/clean-room-policy.md`（禁读上游实现）与路线图 DR-03（停掉上游同步）。

## 1. 结论摘要

| 项 | 值 |
| --- | --- |
| 上游仓库 | `https://github.com/zai-org/ZCode.git` |
| 上游 main（本次审计基准） | `aac4755` = v3.15.1，2026-10-10 18:59 +0800（作者 wuweiqi） |
| 本地基线分支 | `upstream/zcode-baseline` 由 `29628c9`(v3.14.3) 前移至 `aac4755` |
| 两基线差距 | **1 个 squash commit**，11736 文件，+1,942,918 / −25,090 |
| `main` | `70a2f9e4`，未改动 |
| 工作区 | 未改动（仅有会话期间已存在的本地改动，见 §6） |

旧基线 `29628c9acdb81b703bbd4080c207a0e7ce5e276e` 是新基线 `aac4755` 的祖先，随时可 `git branch -f` 回退。

## 2. 重合度（clean-room M4 指标口径：逐字节一致）

按「同路径 + blob SHA 相同」比对：

| 对照 | `main` 文件数 | 同名路径 | **逐字节一致** |
| --- | --- | --- | --- |
| `main` vs v3.14.3（旧基线） | 7837 | 5147（65.7%） | 3295（**42.0%**） |
| `main` vs v3.15.1（新基线） | 7837 | 5140（65.6%） | 3052（**38.9%**） |

- M4 验收目标是该值 = 0%。当前约四成逐字节相同，与路线图"约 56% 上游衍生"的口径同量级。
- 第二档「仅品牌归一后一致」需要 `scripts/upstream-overlap-check.mjs`（M4 产物，**尚未存在**），此处未计算。
- 基线前移后重合度**下降**（42.0% → 38.9%）：上游把改过的文件继续改走了，对自研目标是有利方向。

## 3. 我们独有的文件（上游不存在）：2697 个

这批就是**不能被动到**的自研与改造面：

| 目录 | 文件数 |
| --- | --- |
| `apps/` | 1521 |
| `packages/` | 807 |
| `.agents/` | 179 |
| `scripts/` | 77 |
| `docs/` | 54 |
| `crates/`（`mode-cua-ax` 等） | 29 |
| `.github/` | 10 |
| `harness/`、根目录、`third-party/`、`public/`、`config/`、`.mode/` | 其余 |

代表性自研面：`crates/mode-cua-ax/**`、`packages/mode-cua-plugin/**`、
`apps/mode-cli/packages/node-repl-host/src/cua-broker.ts` / `cua-bridge.ts`、
`mode-protocol-v4/cua-*.ts`、远控（remote-desktop / remote-workspace）与 LAN 链路。

## 4. 上游本次变更分布（按目录，仅新增行）

| 目录 | 文件数 | 新增行 | 删除行 |
| --- | --- | --- | --- |
| `packages/` | 5074 | 894,301 | 13,685 |
| `apps/` | 4158 | 587,950 | 6,785 |
| `docs/` | 2234 | 414,090 | 0 |
| `scripts/` | 75 | 13,201 | 176 |
| `.agents/` | 140 | 12,890 | 3,810 |
| 根目录 / `config/` / `third-party/` / 其他 | 55 | 16,886 | 634 |

文件级：**新增 9835 / 修改 1885 / 删除 15 / 改名 1**。

## 5. 上游演进方向（来自 commit 说明与目录结构）

commit 自述：*Add UI plugins and inline Gen UI experiences; Update desktop, remote workspace,
and agent capabilities; Expand built-in plugins and regression coverage*。

- **仍是 Electron + TypeScript**：上游顶层无 `crates/`，仓库内仅 6 个 tauri 相关资源（material 图标 SVG），
  未见 Rust 化迹象。我们的 Tauri 重写方向与上游不同轨。
- **文档极重**：`docs/` 一次新增 2234 个文件、几乎一功能一文档；顶层为扁平命名（`docs/<feature>.md`），
  与我们 `docs/{specs,plans,community,superpowers}` 的组织方式不同。
- 新增面集中在 `packages/`（UI 插件、inline Gen UI）与 `apps/`（desktop / remote workspace / agent）。

## 6. 需要你注意的两点

1. **上游文档是否算"允许输入"存在口径空白。** clean-room 政策明文禁读的是「上游**实现源码**」；
   上游 `docs/**`（2234 个功能文档）严格说不是实现源码，但政策精神是不从上游取材。
   如果希望把上游文档纳入可读参考，需要显式修订 `docs/specs/clean-room-policy.md`，不要靠默认解释。
2. **`docs/specs/im-bot-onboarding.md` 在本次会话期间出现**（未跟踪），不是本次审计命令产生的，
   判断为你在其它会话/窗口的改动，已原样保留、未触碰。

## 6.5 后续执行：并入上游（branch `sync/upstream-v3.15.1`）

用户明确指示「把能合并的合并、自研代码不能动」后，按三方规则执行，产出提交 `02b588c4`
（11966 文件，+2,287,597 / −2,999）。**仅本地分支，未推送；`main` 未改动。**

| 档 | 文件数 | 动作 |
| --- | --- | --- |
| ADD | 11731 | 上游新增，取入 |
| UPDATE | 235 | 本地未改过、跟进上游新版本 |
| KEEP | 1842 | 本地改过 → 不动 |
| OURS | 2689 | 本地独有 → 不动 |
| DEL_KEEP | 8 | 上游删除 → 本地保留 |

受保护 4539 个文件（KEEP+OURS+DEL_KEEP）提交后复核，与 `main` 逐字节一致。

额外排除（保留本地版本）10 个上游仓库级文件，避免改动本仓库工程设置：
`.husky/pre-push`、`lint-staged.config.mjs`、`package-lock.json`、`vitest.config.ts`、
`vitest.setup.ts`、`.env.development`、`.env.production`、
`packages/desktop/tsconfig.json`、`packages/desktop/tsconfig.scheduler.json`、`packages/shared/tsconfig.json`。

验证（真实结果，未修）：`pnpm typecheck` 2174 errors、`pnpm lint` 92 errors，经文件归属统计
**全部来自并入的上游文件**（`main` 基线为 0 error），属预期的"后续逐步改造"工作。

回退：`git checkout main && git branch -D sync/upstream-v3.15.1`。

## 7. 本次执行过的命令（可复现，均为只读或仅动基线分支）

```
git fetch https://github.com/zai-org/ZCode.git main --no-tags
git branch -f upstream/zcode-baseline aac4755666d09fdcd70272fcf063c077a639015f
git ls-tree -r <ref>            # 逐路径 blob 哈希，用于重合度
git diff --numstat/--name-status <old> <new>
```
