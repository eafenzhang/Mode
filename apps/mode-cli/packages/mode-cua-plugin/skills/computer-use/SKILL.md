---
name: computer-use
description: "Use when the user asks to operate this computer's desktop apps with the mouse and keyboard — clicking, typing, scrolling, reading window/app state, driving a GUI end-to-end, screenshot-based UI verification, or any task phrased as 让 Mode 操作/控制这台电脑、桌面自动化、打开某软件并代为完成操作. Provides the 14 computer-control methods (get_app_state, left_click, type, …) via the node_repl CUA bridge."
when_to_use: "用户要求实际操作本机桌面应用、代为点击/输入/滚动/读取窗口状态，或需要截图验证 GUI 结果时；仅需读代码、查文件或操作浏览器内网页时不使用（网页走 Browser Use）。"
---

# 电脑控制（Computer Use）

通过共享 `node_repl` 的 CUA bridge 驱动本机桌面（Windows 一期；macOS/Linux 当前
fail-closed）。完整参数与错误码规范：插件 docs 根下的 `computer-use.md`
（`bridge.documentationRoot` 指向该 `docs` 目录）。

## 前置

- 设置 →「电脑控制」开关已开启（未开启时 node_repl 不带 CUA 能力，bridge 不存在）。
- 只能在**主会话**使用：子代理里 bridge 会拒绝（subagent 不可用）。
- 首次调用 Helper 按需冷启动：收到 `CUA_NOT_READY`（message 提示稍等重试）→ 稍等后
  **重试同一调用**即可，不要改参数乱试。

## 入口（零依赖，首选）

在 `node_repl` 的 `js` 工具里直接取 bridge global：

```js
const bridge = globalThis[Symbol.for("mode.node-repl.computer-use-bridge")];
if (!bridge) throw new Error("computer-use 不可用：确认设置里电脑控制已开启、当前为主会话");
bridge.assertAvailable();
const result = await bridge.call("get_app_state", { app_ref: { name: "记事本" } });
nodeRepl.write(result);
```

可选的人体工学层——从插件根动态 import 薄 SDK（browser-client 同机制）：

```js
import { pathToFileURL } from "node:url";
const root = bridge.documentationRoot.replace(/[\\/]docs[\\/]?$/, "");
const sdk = await import(pathToFileURL(`${root}/scripts/computer-use-client.mjs`).href);
const client = sdk.createComputerUseClient({ bridge });
const state = await client.observe({ name: "记事本" });
```

## 工作流：观察 → 动作 → 再观察

1. **定位 app**：不知确切标识时先 `list_apps`（拿 name / bundle_id / pid），或直接用
   `app_ref:{name:"<Start 菜单显示名>"}`——未运行会透明启动后回填 pid。
2. **观察** `get_app_state`：读树里 `[<idx>]` 行的 `name = value` 与 `actions` 列表；
   需要看清界面时加 `include_screenshot: true`。树可能带 diff 头，首观察必为全量。
3. **动作**：优先 `{type:"element", index:<idx>}`（索引必须来自**最新一次**观察）；
   树里没有合适元素时用 `{type:"coordinate", x, y}`（绑定最近可动作栅格）。前台要求
   按错误提示处理：`foreground_required` → 改用元素索引或先将窗口带到前台。
4. **再观察验证**：每个有意义的动作之后重新 `get_app_state`，确认状态真的变了；
   不要凭猜测连续动作。
5. **结束**：长时间连续操作后或任务完成时调用 `stop_computer_control` 释放租约
   （幂等）；stop 之后变更类方法会被 `controller_busy` 拒绝，重新控制需再次进入闭环。

## 错误处置速查

- `CUA_NOT_READY` → 稍等重试同一调用。
- `STALE_STATE` / `element_unavailable` → 重新 `get_app_state` 再动，**绝不**盲重放。
- 动作报错但带 `dispatched` 收据 → 输入可能已发出，按「重新观察」处理，不要重复发。
- `controller_busy` → 另一会话占租约或本会话已 stop；观察现状或询问用户。
- `permission_denied`（锁屏）→ 停止尝试，向用户说明。
- `not_settable` / `not_selectable` / `action_unavailable` → 换路径（如坐标、
  `set_value`→聚焦+`type`、`perform_action` 只用树里列出的动作）。

## 安全与边界

- 付款、注册、删除、密码 / 凭据输入等不可逆或敏感操作：先向用户确认再执行。
- 不绕过锁屏；不要把截图里的敏感内容复制到会话之外。
- 网页自动化走 Browser Use 技能；本技能只负责本机原生应用。
