# Spec：Workspace Hook 摘要与快照（Rust `workspace-hook-digest`）

状态：声明/包级摘要 2026-10-29 落地；条目解析与包级快照 2026-10-30 补齐并与 TS 对拍。
**尚未接频道**（`hooks` 频道的 grant 等 W4 的 Agent authority）。

## 1. 范围

本 spec 固定 `crates/workspace-hook-digest` 的四件事：

1. **声明摘要**（`declaration_digest`）：单条 hook 声明的 sha256；
2. **包级摘要**（`bundle_digest`）：全部来源文件 + 全部条目的 sha256；
3. **条目解析**（`resolve_entries`）：把各来源的 `hooks` 配置展开成可审核条目；
4. **包级快照**（`build_bundle_snapshot`）：来源文件列表 + 条目 + 包级摘要。

不在本 spec 内：配置发现与读取（`docs/specs/workspace-hook-config.md`）、trust store
（`crates/workspace-hook-trust`）、settings 模型与 `saveHooks`、频道注册。

## 2. 行为真源

- `packages/shared/src/workspace-hook-digest.ts`：`createWorkspaceHookDeclarationDigest`、
  `resolveWorkspaceHookEntries`、`buildWorkspaceHookBundleSnapshot`。
- 摘要不是对象稳定序列化，而是**构造有序数组后 `sha256(JSON.stringify(array))`**。
- `canonicalOptional`：缺省包成 `["unset"]`、显式值包成 `["set", value]`，
  因此 `undefined` 与 `false` 不会混淆——这是摘要最容易做错的一处，必须逐字节一致。

## 3. 声明摘要

数组形状：

```
["workspace-hook-declaration", 1, sourceRelativePath, sourceDiscoveryOrder,
 event, matcher, matcherIndex, hookIndex, executionPayload,
 resolvedTimeoutMs, resolvedMaxOutputBytes]
```

`executionPayload`：

- `process` → `["process", command, args ?? []]`
- `command` → `["command", command, async === true, shell]`，
  `shell` 为 `["unset"]` / `["true"]` / `["string", value]`。

`sourceRelativePath` = `relative(resolve(workspacePath), resolve(canonicalPath))`，
反斜杠换 `/`，空串回落 basename。

## 4. 包级摘要

数组形状：

```
["workspace-hook-bundle", 1,
 [ [相对路径, discoveryOrder, configFileKind, explicitProjectConfig,
    canonicalOptional(enabled), canonicalOptional(timeoutMs), canonicalOptional(maxOutputBytes)] … ],
 [ [hookDeclarationDigest, sourceRootEnabled, declarationEnabled,
    runtimeHooksEnabled, configuredEnabled] … ]]
```

来源顺序即发现顺序，条目顺序见 §5；任何重排都会让同一份配置算出两个摘要、信任记录失效。

## 5. 条目解析（`resolve_entries`）

遍历顺序：`sourceFileIndex` → `WORKSPACE_HOOK_EVENT_NAMES`（固定 7 个，顺序固定）→
`matcherIndex` → `hookIndex`。

- `reviewItemId` = `workspace-hook-{sourceFileIndex}-{event}-{matcherIndex}-{hookIndex}`。
- 门禁位：`sourceEnabled = source.hooks.enabled`、`declarationEnabled = hook.enabled`、
  `runtimeHooksEnabled = runtimeRoot.enabled`，三者按「缺省为开」归并出 `configuredEnabled`。
- `resolvedTimeoutMs`：`hook.timeoutMs` →（`command` 且 `hook.timeout` 有值）×1000 →
  `runtimeRoot.timeoutMs`，再 `max(1, round)`。
- `resolvedMaxOutputBytes` 取 `runtimeRoot.maxOutputBytes`（不看来源级设置）。
- 输出字段按类型裁剪：`process` 只带非空 `args`；`command` 只带
  `async: true`（false 不输出）与存在的 `shell`；`statusMessage` 仅在非空时输出。

## 6. 包级快照（`build_bundle_snapshot`）

- 条目为空 → 返回 `None`（TS 返回 `undefined`）。
- `sourceFiles` 逐份来源输出 `canonicalPath` / `baseDir` / `discoveryOrder` /
  `configFileKind` / `explicitProjectConfig` / `editable` 与 `hooksRoot`
  （只保留定义过的 `enabled` / `timeoutMs` / `maxOutputBytes`）。
- `discoveredAt` 由调用方传入（TS 默认 `new Date().toISOString()`）。
- 顶层：`schemaVersion=1`、`workspaceIdentity`、`discoveredAt`、`sourceFiles`、`hooks`、
  `digestAlgorithm="sha256"`、`bundleDigest`。

## 7. 已知差异

- 路径规范化用词法归约（折叠 `.`/`..`），不访问文件系统；对已存在路径与 Node
  `path.resolve/relative` 一致。
- `deepFreeze` 语义在 Rust 侧不存在（所有权即可保证不可变）。
- `discoveredAt` 不内置时钟：壳内调用方负责取当前时间。

## 8. 验收

```bash
pnpm hook-snapshot:fixtures                                    # 条目 + 快照
pnpm hook-digest:fixtures                                      # 既有摘要黄金字节
cmd //c "D:\tools\cargo-msvc.cmd test -p workspace-hook-digest"
```

`fixtures/hook-snapshot.json` 覆盖 8 组纯数据用例：基本组合、来源级关闭、条目级关闭 +
运行时根关闭、`shell`/`async` 变体、超时优先级（秒级换算、`timeoutMs` 覆盖、进程类型
忽略 `timeout`、0.4 取整到 1）、两份来源（显式 + 可编辑）、嵌套相对路径、空配置
（条目为空且无快照）。比较项为条目 JSON 与完整快照 JSON（含两个摘要的十六进制值）。
