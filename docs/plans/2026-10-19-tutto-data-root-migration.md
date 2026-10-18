# Tutto：用户数据根 `.mode → .tutto` 迁移清单（2026-10-19）

> 属于 P3 spec S3；涉及用户数据，必须单独窗口执行并验证迁移失败回退。
> 当前契约：`packages/shared/src/data-root.ts`。

## 1. 常量切换

| 常量 | 现在 | 改成 | 兼容 |
| --- | --- | --- | --- |
| 数据根目录 | `.mode` | `.tutto` | 旧根 `.mode` / `.zcodium` / `.zcode` 仅迁移 |
| 归属文件 | `.mode-root.json` | `.tutto-root.json` | 读旧文件判定历史根 |
| 产品 id | `dev.mode.app` | `dev.tutto.app` | 接受 `dev.mode.app` / `dev.zcodium.app` |
| 迁移动作 | copy 旧根 → 新根 | 不变 | 失败保留旧根，不回写、不删除 |

## 2. 需要同步的落点

- `packages/shared/src/data-root.ts`（常量、legacy 列表、`isAcceptedDataRootProduct`）；
- `packages/services` / `packages/desktop` / `apps/mode-cli` 中写死 `.mode` 的路径与提示文案；
- Electron userData 已改为 `Tutto`（S1），需确认与数据根迁移互不依赖；
- oracle 脚本：`isolation.test.mts`、`probe-plugin-roots.mts`、`probe-plugin-sidecar.mts`、
  `probe-bash-background.mts`、`probe-write-support.mts` 等把 `.mode` 改为 `.tutto`；
- 文档：`docs/specs/data-root*`、README 数据目录说明、迁移提示 i18n。

## 3. 迁移语义（不得更改）

1. 新根不存在、旧根存在 → copy 旧根到新根，写新归属文件；
2. 新旧都存在 → 用新根，不合并、不覆盖；
3. copy 失败 → 继续用旧根（`fallback`），UI 提示真实路径；
4. 旧根永不删除；用户手动清理。

## 4. 测试矩阵

- 全新环境：创建 `.tutto` + `.tutto-root.json`；
- 仅 `.mode`：迁移后设置/机器人/凭证/插件仍在；
- 仅 `.zcodium`/`.zcode`：多代迁移仍可达；
- `.tutto` 已存在且 `.mode` 也存在：优先新根，旧根不动；
- 归属产品 id：`dev.tutto.app`、`dev.mode.app`、`dev.zcodium.app` 均接受，未知名拒绝；
- copy 失败注入：回退旧根并可继续启动。

## 5. 验证

`pnpm typecheck`、`pnpm --filter @mode/shared test`、`pnpm --filter @mode/services test`、
desktop CLI 的 data-root 相关测试、`pnpm architecture:check --changed`、语言政策；
本地提交，不推送。

## 6. 风险

- 迁移方向写反会造成「新根空、旧数据看不到」；
- 归属文件产品 id 校验漏旧值会让历史根被判 unowned；
- oracle 脚本漏改会把测试数据写进真实 `.mode`。
