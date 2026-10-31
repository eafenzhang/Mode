# Spec：Git repo 层（Rust `git-service`）

状态：2026-10-31 首版。对应 `packages/services/src/git/repo/gitCliRepo.ts` 与
`gitService.ts` 门面。命令执行与解析在 `git-cli`（见 `docs/specs/git-cli.md`）；
`git` 频道已注册（默认不接管）；`git-checkpoint` 见 `docs/specs/git-checkpoint-service.md`。

## 1. 范围

| Rust                                         | 说明                                                                                                                        |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `resolve_repository`                         | `rev-parse --show-toplevel/--show-prefix/--absolute-git-dir/--git-common-dir`，含不可用/非仓库降级                          |
| `get_workspace_repository_info`              | main-tree / linked-worktree（`.git` 文件指向 `/.git/worktrees/`）                                                           |
| `get_status`                                 | status v2 + staged/unstaged numstat + 未跟踪行数；输出超限后按 repoRoot 折叠未跟踪目录并重试                                |
| `get_ignored_paths`                          | `check-ignore`（退出码 1 视为全部未忽略）                                                                                   |
| `list_local_branches`                        | `for-each-ref refs/heads`（当前分支优先、时间倒序、名称升序）                                                               |
| `get_commit_graph`                           | `log HEAD --branches --tags --remotes`，maxCount 夹在 1..=200，多取一条判 `hasMore`                                         |
| `switch_branch` / `create_branch_and_switch` | 冲突 / 进行中操作 / 分支名校验在前，失败归一为 issue，成功后刷新状态                                                        |
| `get_diff`                                   | unstaged / staged / branch（`upstream...HEAD`）三类 patch；文本全文预览（blob 与工作区）成对成功才附加                      |
| `get_branch_comparison`                      | `upstream...HEAD --numstat`                                                                                                 |
| `stage` / `unstage` / `discard`              | 路径规范化去重后执行 `add` / `restore --staged` / `restore`                                                                 |
| `commit`                                     | 普通提交与 `stagedOnly` 临时 index 提交（read-tree → update-index → commit → reset）                                        |
| `push`                                       | 已跟踪分支直接 push；否则解析 remote（branch.<name>.remote → remote.pushDefault → origin → 唯一 remote）后 `--set-upstream` |
| `get_identity`                               | `config --show-scope --show-origin --get user.name/user.email`                                                              |

## 1.1 门面（`gitService.ts`）与频道

`GitService` 在 repo 快照之上做频道类型映射：

- `get_changes(source_id)`：按 workspace 范围裁剪后生成 `GitFileChange`；
  staged 只收 `x` 非空非 `.` 且非未跟踪/冲突项；unstaged 区分 conflicted / untracked
  （空文件与目录占位规则：路径以 `/` 结尾或行数 > 0 才保留）/ 普通修改；
- `get_branch_comparison` / `refresh`：branch 变更映射为 `section: "branch"`；
  `refresh` 复用同一份 status 快照切出 summary/identity/unstaged/staged/branchComparison；
- `commit`：返回 `{ commitHash, summary }`（提交后重新取状态）；
- `generate_commit_message`：依赖 Agent 模型调用（W4），壳内返回与 TS 无 generator 时
  相同的错误 `Commit message generation is not available.`；
- 频道 `git`：18 个方法，参数为位置参数对象，Git 命令在 `spawn_blocking` 中执行。

## 2. 与 TS 的已知差异

1. **无 in-flight 请求缓存**：TS 用 Promise 复用并发解析/状态；Rust 同步实现每次重新执行
   （结果一致，去掉并发去重；`invalidate` 保留为空实现）。
2. **未跟踪折叠状态**：TS 在实例内按 repoRoot 记住“输出超限后折叠”，Rust 同样保存在实例
   的 `Mutex<BTreeSet>` 中；实例生命周期内的行为一致。
3. **`signal` 字段**：命令结果不含信号名（见 `git-cli` spec）。
4. **临时 index 目录**：`mode-git-index-<pid>-<nanos>`（TS 用 mkdtemp 随机串）。
5. **错误文本**：Git stderr 原样透传；`ensure*` 前缀逐字对齐。
6. **提交信息生成**：`generateCommitMessage` 在壳内恒返回
   `Commit message generation is not available.`（W4 接入 Agent 模型后补齐）。

## 3. 验收

- `pnpm git-service:fixtures` 生成 `crates/git-service/fixtures/git-service.json`：
  7 个用例在真实 git 仓库上执行（固定作者/时间、隔离全局配置、本地 bare 远端），
  覆盖 status/diff（含 branch 与未跟踪）/ignored/branches/graph/identity/仓库类型/
  分支切换与创建（含 invalid/不存在）/暂存与提交（含 stagedOnly）/丢弃/推送/
  分支比较/门面 changes+refresh+commit/提交信息不可用/非仓库/游离 HEAD。
- `cargo test -p git-service` 在 Rust 侧重建同样仓库并逐项深比较；40 位 hash 归一为
  `<hash>`，沙盒根归一为 `<root>`；fixture 重复生成 sha256 一致。
- `cargo test -p git-cli` 的解析器对拍（39 op）保持全绿。
