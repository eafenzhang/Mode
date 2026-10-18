# Tutto：插件清单目录 `.mode-plugin → .tutto-plugin` 迁移清单（2026-10-19，已完成）

> 目标：新清单目录 `.tutto-plugin` 为主，读取旧 `.mode-plugin` / `.zcode-plugin` 兜底；
> 新建/打包/seed 只写新名。属于 P3 spec S4 的一个原子任务，**一次提交完成**。

## 1. 随包清单目录（git mv）

```bash
git mv apps/mode-cli/packages/browser-use-plugin/.mode-plugin \
       apps/mode-cli/packages/browser-use-plugin/.tutto-plugin
git mv apps/mode-cli/packages/mode-cua-plugin/.mode-plugin \
       apps/mode-cli/packages/mode-cua-plugin/.tutto-plugin
git mv apps/mode-cli/packages/node-repl-host/.mode-plugin \
       apps/mode-cli/packages/node-repl-host/.tutto-plugin
```

## 2. 解析点（29 个文件）

| 组 | 文件 | 改法 |
| --- | --- | --- |
| adapters/plugins | `index.ts`、`marketplace.ts`、`zip-source.ts` | 常量切 `.tutto-plugin`；候选链加 `.mode-plugin`、`.zcode-plugin` |
| adapters/skills | `skills/index.ts` | 清单相对路径数组改为「新名优先 + 旧名」 |
| bootstrap | `bundled-plugins.ts`、`official-plugin-runtime.ts` | 白名单/根探测/`file.find` 路径改新名，旧名兜底 |
| CLI 构建 | `sea-official-plugin-assets.mjs` | SEA 资产路径改新名，旧名兜底 |
| desktop 打包 | `prepare-agent-node-bundle.mjs` | 随包清单候选与 `manifestPath` 探针改新名 |
| server | `modeAgentDevDeploy.ts`、`modeAgentOfficialPluginAssets.ts` | 开发部署与远端资产路径改新名 |
| services | `commandsService.ts`、`skillsService.ts`、`settingsSyncService.ts`、`subagentsService.ts`、`pluginSyncService.ts` | 常量改新名；候选链加 `.mode-plugin`、`.zcode-plugin` |
| sync | `pluginSyncArchive.ts` | 只涉及 `.tutto-plugin-sync.json`（已完成），勿改 |
| oracle fixtures | `probe-plugin-*.mts`（6 个） | 夹具目录改 `.tutto-plugin` |
| 测试 | `cua-plugin-distribution.test.ts`、`official-plugin-attribution.test.ts` | 断言路径改新名 |
| 构建/其它 | `scripts/prepare-prebuilds.mjs`、`tools/plugin-host/src/main.ts`、`mode-cua-plugin/README.md` | 路径/文档改新名 |
| 历史脚本 | `scripts/p2-mode-rename.mjs` | **不改**（记录历史映射） |

## 3. 兼容口径

- 读取：`.tutto-plugin/plugin.json` → `.mode-plugin/plugin.json` → `.zcode-plugin/plugin.json`；
- 写入/seed/打包：只写 `.tutto-plugin`；
- 第三方插件目录不迁移，只靠读取兜底。

## 4. 验证

1. `pnpm typecheck`、`pnpm --filter @mode/services test`、`pnpm --filter @mode/desktop test`（oracle 相关）、
   `pnpm architecture:check --changed`、语言政策；
2. `git grep "\.mode-plugin"` 只剩兼容列表、历史脚本与 sync 文件名；
3. 抽查随包插件：browser-use / node-repl-host / mode-cua 的 manifest 在新目录可被发现；
4. 本地提交，不推送。

## 5. 风险

- 任一解析点漏改会导致「随包插件不可发现」或 SEA/远端资产缺清单；
- 旧目录仍在缓存/已安装插件里，必须保留读取兜底至少一个发布周期。

## 6. 执行结果（2026-10-19）

- 三个随包插件清单目录已 `git mv` 到 `.tutto-plugin`；
- 解析点全部切换并保留 `.mode-plugin` / `.zcode-plugin` 读取兜底；
- services 379（371 pass / 8 skip）、adapters 13、bootstrap 30 全绿；
- `git grep ".mode-plugin"` 剩余仅兼容列表、历史脚本与旧同步文件名。
