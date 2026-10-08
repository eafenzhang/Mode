# 远程 workspace identity 构造与解析契约

## 产品规则

- **唯一入口**：构造与解析只允许走 `packages/shared/src/remote-workspace-identity.ts` 的
  `buildRemoteWorkspaceIdentity` / `parseRemoteWorkspaceIdentity`。Host、Main、UI 禁止手写
  拼接或拷贝归一化实现（UI 曾因手拷贝 normalize 与契约漂移，产出 `remote:lan:…:D:/path`
  无前导斜杠形态，对端 fail-closed 拒绝 → 发送消息报 `fault.command.executionFailed`）。
- **段格式**：

```text
remote:ssh:<host>:<port>:<username>:<posixPath>
remote:wsl:<distro>[:<user>]:<posixPath>
remote:docker:<container>:<posixPath>
remote:lan:<host>:<port>:<path>        ← path 见下
```

- **lan 的 path 双形态（本 spec 核心）**：
  - Windows 对端：规范形态 = **盘符开头** `D:/gas-hub-xp`（不加前导 `/`）。实测
    `/D:/…` 在 Windows 上作工作目录为 ENOENT，前导斜杠规则是 posix 假设，对
    Windows 间 LAN 目标不可用；报错野外形态即此形态，直接接纳。
  - posix 对端（mac/Linux）：照旧以 `/` 开头。
  - 解析侧对 `lan` 同时接受三种历史形态并归一：`D:/…`（规范 / 野外）、`/…`（posix）、
    `/D:/…`（今天上午的旧规范形态，读出时去前导斜杠归一为 `D:/…`），保证已绑定会话
    不需要重绑即可恢复。
  - 其余 kind（ssh/wsl/docker）维持 posix 规则：路径必须以 `/` 开头，否则 fail-closed。
- **fail-closed**：`remote:` 前缀但解析失败的身份绝不能落回「按本地路径处理」，
  必须抛 `Invalid remote workspace identity`（`resolveWorkspaceRefFromId`）。
- 路径归一：`\` → `/`、折叠重复分隔符、去收尾斜杠；盘符形态保留 `X:/` 前缀。
- 存量影响：已持久化的 identity 字符串形态不迁移（identity 本身就是 workspaceKey，
  改形态会导致历史条目失联）；构造器输出形态变更只影响**新绑定**。

## 验收场景

1. `buildRemoteWorkspaceIdentity("D:\\Work\\Demo\\", lanTarget)` →
   `remote:lan:192.168.1.20:45880:D:/Work/Demo`，解析往返一致。
2. 解析野外形态 `remote:lan:100.68.49.120:45880:D:/gas-hub-xp` →
   `{kind:"lan", workspacePath:"D:/gas-hub-xp"}`（此前为 null，即本次故障）。
3. 解析旧规范形态 `remote:lan:h:p:/D:/Work/Demo` → `workspacePath:"D:/Work/Demo"`（归一）。
4. posix lan 路径 `/home/dev/proj` 构造与解析不变；相对路径 `x`、缺段仍拒绝。
5. ssh/wsl/docker 的 `/` 强制与 wsl 可选 user 判别行为不变（既有测试全绿）。
6. UI 删除本地 `normalizeWorkspacePathForIdentity`/`buildRemoteWorkspaceIdentity`，
   改调 shared；`pnpm typecheck` 全绿。

## 不做

- ssh 指向 Windows 主机的盘符身份（无此场景，保持 posix 规则）。
- 已持久化 identity 的批量迁移。
