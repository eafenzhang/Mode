# Spec：发布验证门禁

状态：2026-10-28 实现。关联：
`.github/workflows/release.yml`、`.github/workflows/sqlite-schema.yml`、
`package.json` 的 `verify:pre-push`。

## 目的

发布流水线的 `verify` job 是公开安装包之前的最后一道自动门禁。它必须覆盖「应用能构建」
之外、但会导致用户升级后不可用的契约：CLI 类型正确、数据库 schema 只增。

## 门禁项

1. **CLI 门禁**：`pnpm check:cli`（等价于 `pnpm --dir apps/mode-cli check`）。
   - 先跑 `registry:check`，再跑 CLI 各包的 `turbo run typecheck`。
   - 根 `pnpm typecheck` 不包含 `apps/mode-cli`，因此不能替代本项。
   - 注册表哈希把包内相对路径归一为 `/`，Windows 的 `\` 与 Linux 的 `/` 不再产生
     不同 hash；`--check` 比较前统一换行符为 LF，Windows 的 CRLF 检出不会误报过期。
     因此 `registry:check` 在两个平台对同一份 `@withfig/autocomplete` 结果一致。
2. **SQLite schema 门禁**：`pnpm check:sqlite-schema`。
   - 默认 check 要求当前迁移快照与 `scripts/sqlite-schema-baseline.json` 一致；
   - 独立的 `sqlite-schema` workflow 带 `--base-ref`，阻止「删列 + 改基线」绕过；
   - 规则见 [sqlite-schema-gate.md](./sqlite-schema-gate.md)。
3. **本地 pre-push**：`pnpm verify:pre-push` 增加 `pnpm check:sqlite-schema`。
   - 本地检查不传 `--base-ref`，只保证工作树与基线一致；
   - 架构检查、Lint、语言策略保持现有顺序。

## 所有者与失败语义

- `release.yml` 的 `verify` 是发布门禁唯一执行者；任一项失败 → `verify` 失败 →
  `tauri`/`publish` 不执行，Release 保持 draft。
- 门禁不自动修复、不自动更新基线；失败输出必须包含具体命令与下一步。
- 新增门禁不得放宽已有检查，也不得用 `continue-on-error` 吞掉失败。

## 验收

1. 在 `apps/mode-cli` 注入一个类型错误，`pnpm typecheck` 仍通过，但
   `pnpm --dir apps/mode-cli check` 与 release `verify` 失败。
2. 删除一个已存在列，`pnpm check:sqlite-schema` 失败；恢复后通过。
3. 新增列但不更新基线，check 失败；执行 `pnpm sqlite-schema:update` 并提交后通过。
4. `verify:pre-push` 在基线过期时失败，不进入推送。
