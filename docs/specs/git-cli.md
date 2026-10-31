# Spec：Git CLI 基础层（Rust `git-cli`）

状态：2026-10-31 首版。对应 `packages/services/src/git/config.ts`、
`providers/gitCommandProvider.ts`、`providers/gitEnvironmentProvider.ts`、
`repo/gitCliHelpers.ts`、`repo/gitCliTypes.ts`。`git` 与 `git-checkpoint` 两个频道
共用本 crate；repo 层与频道适配器尚未迁移。

## 1. 范围

- **命令执行**：按候选列表探测 `git`（`MODE_GIT_BINARY` → `git` → Windows 常见安装路径），
  一次会话内缓存首次成功结果；执行时清理 Git local env（`GIT_DIR` 等 16 个变量、
  防止 hook 串仓），叠加 `GIT_OPTIONAL_LOCKS=0`、`GIT_PAGER/PAGER=cat`、`TERM=dumb`、
  `LC_ALL/LANG=C`。
- **超时与输出配额**：默认命令 15s、diff 20s、push 10min；输出 512 KiB、push 8 MiB、
  diff 1 MiB。超时先 `kill`，等待 2s 后再强制结束（Windows 用 `taskkill /PID … /T /F`），
  再等 2s 仍不退则标记 `orphaned` 并按无退出码返回；输出超限标记 `outputTruncated`
  并结束进程。spawn 失败（含 cwd 被删）降级为 `exitCode=-2` 的结果，不抛异常。
- **解析器**：`git status --porcelain=v2 -z`（header / `?` / `1` / `2` / `u`）、
  `git diff --numstat -z`（含重命名三段式与 `-` 计数）、`git config --show-scope
--show-origin --get`、分支切换/创建的错误归一（10 种 issue code）、单文件 diff
  结果分类（patch/binary/truncated/unavailable）、未跟踪文件行数统计与 unified diff
  预览、仓库内路径归一与工作区范围判断。

## 2. 与 TS 的关键口径

- **status v2**：`1` 记录为 XY + 6 字段 + 路径（路径可含空格）；`2` 为 XY + 7 字段 +
  路径，随后一条 record 是原路径；`u` 为 XY + 8 字段 + 路径；字段不足或未知记录跳过。
  `branch.head (detached)` → `headRefType: "detached"`；`branch.ab ±N` 解析 ahead/behind。
- **numstat**：路径字段为空时，下两条 record 分别是原路径与新路径；`-` 计为 0。
- **diff 分类顺序**：超时 → truncated → 退出码 → 空输出 → 二进制（`GIT binary patch`
  或 `Binary files `）→ patch。
- **未跟踪统计**：文件 >1 MiB、含 NUL 或读取失败 → 0 行；按最后字节判定末尾换行。
- **路径**：`normalizeGitPath` 反斜杠转 `/`；工作区范围比较基于 `/` 归一后的前缀；
  `normalizeInputPath` 解析 realpath 后必须仍在 repoRoot 内（`..` 越界报
  `Path is outside repository scope: <path>`）。

## 3. 已知差异

1. **信号**：Node 结果携带 `signal`（SIGKILL 等）；Rust 只保留 `None`。上层只使用
   `timedOut` / `exitCode`，无行为差异。
2. **未跟踪统计并发**：TS 用 4 个 worker 并行读取；Rust 顺序读取（统计结果一致，
   大仓库耗时略高）。
3. **Windows 相对路径**：`path.relative` 在 Rust 侧按组件大小写不敏感比较近似；
   正常盘符路径一致，跨盘符会得到 `..` 形式并触发越界错误（与 Node 一致）。
4. **错误文本**：spawn 失败与超时的细节字段取自 Rust 运行时；`ensure` 文案逐字对齐 TS。

## 4. 验收

- `pnpm git-cli:fixtures` 生成 `crates/git-cli/fixtures/git-cli.json`（6 个用例、39 个
  op：status/numstat/config/issues/diff/paths，全部为纯解析器调用）。
- `cargo test -p git-cli` 回放并逐项深比较；另有 7 条单元测试覆盖执行器的
  二进制缺失、解析边界与 diff 分类。
