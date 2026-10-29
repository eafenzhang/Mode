# Spec：workspace 身份归一与 hash（`crates/workspace-identity`）

状态：2026-10-29 实现（W1 基元）。行为真源：
`packages/shared/src/remote-workspace-identity.ts` 与 `packages/services/src/paths.ts`。

## 两个概念不能混用

- `workspacePath`：用于文件操作、命令 cwd、Git、路径展示。
- `workspaceIdentity`：用于去重、绑定、缓存、队列、持久化与请求关联。
- 身份 key = `workspaceIdentity?.trim() || workspacePath`（AGENTS.md「Workspace Identity」）。

## 路径归一（`normalize_workspace_path_for_identity`）

1. 反斜杠 `\` → `/`；
2. 连续 `/` 合并成一个；
3. 去掉首尾 `/`；
4. 若结果匹配 `^[A-Za-z]:(/|$)`（Windows 盘符），保留 `X:/` 形态；
5. 否则补一个前导 `/`（因此空串归一为 `/`）。

例：`D:\ZCodium\` → `D:/ZCodium`；`/home//user/` → `/home/user`；`relative/path` →
`/relative/path`；`//D:/x` → `D:/x`；`C:` → `C:`。

## hash

`workspace_hash(key)` = SHA-256(key) 的十六进制前 **12 位**，与 Mode session 持久化目录
（`{appConfigDir}/sessions/<hash>`）和对话镜像目录
（`{appConfigDir}/conversation-mirror/<hash>`）一致。

`workspace_hash` 接收的是**身份 key**，不是路径：对话镜像的调用方式是
`workspace_hash(normalize_workspace_path_for_identity(workspacePath))`（不带 identity）；
会话目录的调用方式等价于 `workspace_hash_for(workspacePath, identity)`。

## 验收

`crates/workspace-identity` 的 3 条单测覆盖归一分支、identity 优先级与 4 个 hash 期望值
（期望值由 Node `createHash("sha256")…slice(0,12)` 生成）。`clippy -D warnings` 通过。
