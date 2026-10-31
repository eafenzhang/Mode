# Spec：Git workspace checkpoint（Rust `git-checkpoint-service`）

状态：2026-10-31 首版。对应 `packages/services/src/git/gitCheckpointService.ts` 与
`repo/gitCheckpointRepo.ts`、`repo/gitCheckpointStore.ts`、`repo/gitCheckpointHelpers.ts`。
命令执行复用 `git-cli`（见 `docs/specs/git-cli.md`）；`git-checkpoint` 频道已注册（默认不接管）。

## 1. 状态所有权

| 状态                   | 所有者                                | 位置                                                        |
| ---------------------- | ------------------------------------- | ----------------------------------------------------------- |
| checkpoint manifest    | `GitCheckpointStore`（本 crate 独占） | `<dataRoot>/v2/checkpoints/<workspaceHash>/<id>.json`       |
| 隐藏 commit / 隐藏 ref | Git 仓库                              | `refs/mode/checkpoints/<workspaceHash>/<id>`                |
| 临时 index             | 单次 create 调用                      | `<dataRoot>/git-checkpoint-index/index-<pid>-<nanos>/index` |

- `workspaceHash` = `workspace-identity` 的 SHA-256 前 12 位（key 规则与 AGENTS.md 一致，
  checkpoint 侧只传 `workspacePath`，与 TS `getWorkspaceHash` 默认调用一致）。
- manifest 写临时文件后 rename；进程内写锁串行化（TS 用 per-file Promise 链）。
- 临时 index 在 create 结束时无条件删除（成功与失败都删）。

## 2. 方法

| 频道方法                                                             | Rust                          | 说明                                                                                                                                                                                                                                                 |
| -------------------------------------------------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `createCheckpoint({workspacePath})`                                  | `create_checkpoint`           | 解析仓库 → 预热临时 index（复制真实 index，失败降级 `read-tree HEAD`，再失败用空 index）→ `add -A -- <pathspec>` → `write-tree` → `commit-tree`（作者/提交者固定 `Mode Checkpoint <checkpoint@mode.local>`）→ `update-ref` → 写 manifest             |
| `diffCheckpoints({workspacePath, fromCheckpointId, toCheckpointId})` | `diff_checkpoints`            | `diff --name-status/--numstat --find-renames -z <from> <to> -- <pathspec>`，合并为 `GitCheckpointDiff`（rename 的 numstat 同时记到新旧路径）                                                                                                         |
| `restoreBetweenCheckpoints({..., force?})`                           | `restore_between_checkpoints` | 三方语义：先算 from→to 受影响路径，再校验磁盘仍等于 from；冲突且未 force 时返回 `{success:false, conflicts}`；否则 `restore --source=<to> --worktree` 并删除 deleted/rename 旧路径，最后按 to 复验，失败抛 `Checkpoint restore verification failed.` |
| `deleteCheckpoint({workspacePath, checkpointId})`                    | `delete_checkpoint`           | 先载入 manifest（不存在/workspace 不匹配即报错），删隐藏 ref（退出码 0/1 允许），再删 manifest                                                                                                                                                       |

`workspacePath` 只用于定位仓库与 manifest；`pathspec` 取 `workspaceInRepoPath`（`.` 保持 `.`），
所有 diff/restore/冲突检测都收敛在该 scope 内，monorepo 子目录不会触碰仓库其它部分。

## 3. 错误语义（逐字对齐）

- `Checkpoint does not exist: <id>`：manifest 缺失或字段校验失败（`scope` 必须为 `workspace`）。
- `Checkpoint workspace mismatch: <id>`：manifest 的 `workspacePath` 与请求不一致。
- `Checkpoint repoRoot mismatch.` / `Checkpoint scope mismatch.` /
  `Checkpoint workspace scope mismatch.`：两个 checkpoint 不可比较。
- `Git binary is not available in the current environment.` / `Workspace is not inside a Git repository.`
- 冲突原因：`unexpected-file-in-worktree`（from 中无此路径但磁盘存在）、
  `missing-in-worktree`（from 要求存在但磁盘缺失）、`type-mismatch`（期望文件/符号链接却遇到目录或类型不符）、
  `content-mismatch`（`hash-object --no-filters` 与 from 的 blob 不一致）；
  同一路径多次命中时保留后一次。

## 4. 与 TS 的已知差异

1. **checkpoint id**：TS `randomUUID`，Rust `uuid::Uuid::new_v4()`，同为小写带连字符。
2. **临时 index 命名**：`index-<pid>-<nanos>`（TS 用 `mkdtemp(resolve(root, "index-"))`）。
3. **写串行化粒度**：Rust 单把进程内 Mutex（store 全局）；TS 按文件 Promise 链。
   两者都保证同一 manifest 不并发写，Rust 侧更保守。
4. **`removeFileIfExists`**：Rust 先 `remove_dir_all` 再 `remove_file`、错误忽略，等价于
   `rm -rf force`。
5. **verify 与过滤器**：与 TS 一致使用 `hash-object --no-filters` 对比 blob；仓库开启
   `core.autocrlf` 时 restore 写回的是转换后字节，复验会报 `Checkpoint restore verification failed.`
   ——这是 TS 同款行为，移植时保持不变（fixture 与频道测试隔离全局配置）。
6. **并发**：TS 的 `Promise.all` 两个 checkpoint 载入、diff 的两条命令并行；Rust 串行。
   结果与顺序无关。

## 5. 验收

- `pnpm git-checkpoint:fixtures` 生成 `crates/git-checkpoint-service/fixtures/git-checkpoint-service.json`：
  7 个真实 git 仓库用例，覆盖 create/diff（modified/added/renamed）、restore 成功、
  冲突（content-mismatch + unexpected-file-in-worktree）与 force、删除后引用与 manifest 同时消失、
  嵌套 workspace scope（repo 外文件不受影响）、非仓库、缺失 checkpoint。
- `cargo test -p git-checkpoint-service` 重建同构沙盒并逐项深比较；归一化口径：
  sandbox → `<root>`、checkpoint id → `<checkpoint-N>`、`createdAt` → `<ts>`、
  40 位 oid → `<hash>`、ref 内 12 位 hash → `<workspaceHash>`；fixture 重复生成 sha256 一致。
- `cargo test -p host-channels --test git_checkpoint_channel`：频道参数形状、camelCase 返回、
  `deleteCheckpoint` 返回 `undefined`、缺失 checkpoint 与未知方法错误映射。
