# @mode/cua-plugin — Computer Use（电脑控制）插件种子资产

本包是官方插件 `computer-use@mode-plugins-official` 的 producer：**只携带 skill、docs 与
SDK 三类静态资产，不声明 mcpServers、不携带 native runtime**（宿主契约见
`bootstrap/src/app/official-plugin-runtime.ts` 的 rewrite 注释：mode-cua 为 skill/SDK-only，
工具面由共享 `node_repl` 承载）。

- 契约与分发接线：`docs/specs/computer-use-plugin-distribution.md`
- 工具面与 runtime 规范：`docs/specs/computer-use-windows-runtime.md`
- 开关权威门：`docs/specs/computer-use-enablement.md`

## 布局

| 路径                              | 作用                                                                            |
| --------------------------------- | ------------------------------------------------------------------------------- |
| `.mode-plugin/plugin.json`        | 发现清单（`name` 必须为 `computer-use`，版本对齐官方定义，**禁止 mcpServers**） |
| `docs/computer-use.md`            | 14 个方法的工具面规范；宿主经 `MODE_CUA_PLUGIN_ROOT/docs` 暴露给会话            |
| `skills/computer-use/SKILL.md`    | 技能：教模型通过 node_repl 的 CUA bridge 驱动本机                               |
| `scripts/computer-use-client.mjs` | 自研薄 SDK：方法名常量 + bridge 绑定（kernel 可从插件根动态 import）            |

## seed 与打包

- dev：`resolveFilesystemPluginRoot` 经 bootstrap 模块目录回退 `../../../mode-cua-plugin` 命中本包。
- 打包：`prepare-agent-node-bundle.mjs` / `prepare-prebuilds.mjs` 把本包 stage 到
  `packages/mode-cua-plugin`（entrypoint 相对路径），顶层白名单见
  `bundled-plugins.ts` `includedTopLevelPaths`。
- 版本改动必须同步 `official-plugin-definitions.ts` 的 `version` 与市场条目，否则
  `comparePluginUpdate` 会永久报「可更新」。
