# Composer 模型选择所有权与草稿初始化

## 背景

v4 Composer 的“下一次提交配置”（模式、模型、推理档位）由 Renderer 草稿拥有，
不直接取自会话投影。但草稿的首次初始化又依赖会话 Snapshot（`sessionConfig`）：
当草稿没有 `mode` 时，`useDraftConfigControl` 会把 Snapshot 里的
`modelSelection` 写进草稿。

在会话 Snapshot 到达前，模型选择器已经可用（模型目录是全局的）。如果用户此时
选了一个模型，草稿里只有 `modelSelection`、没有 `mode`；随后 Snapshot 到达，
初始化分支会用会话旧值（或空值）覆盖刚选中的模型。对于已有会话，点击时还会同时
发出 `switchModelConfig`，于是出现“界面被改回旧模型、下一条消息又切回去”的
自动改变现象。

同时，设备对话（`ModeAgentChatSidePane`）在未创建会话时传入独立的
`draftScopeId`，但配置 owner 只认 `sessionId ?? "__draft__"`，导致它与主
新任务面板共享同一份草稿，模型/正文会互相覆盖。

## 状态所有权

- **Composer 草稿**（`V4ComposerDraft`）：唯一拥有下一次提交的
  `mode`、`planEnabled`、`modelSelection`。按 `(workspaceKey, draftScopeId)`
  隔离；`draftScopeId = sessionId ?? 面板草稿 scope`。
- **会话 Snapshot**：只提供初始化种子，不覆盖用户已经显式写入的草稿字段。
- **ModelSelectionView**：只做候选、校验和有效值投影；展示层不得把投影结果写回
  草稿，也不得因为暂时无法解析就删除草稿里的意图。
- **Host/runtime**：`switchModelConfig`、提交 intent 是另一条执行事实；草稿不回读
  它来修正模型身份。

## 规则

1. 初始化分支只在 `draft.mode` 缺失时执行；执行时：
   - `mode`、`planEnabled` 取自种子（新任务初始化或会话 Snapshot）；
   - `modelSelection` 已有值时保留草稿值，只有草稿为空时才采用种子。
2. 用户通过模型/档位/模式控件显式修改配置后，后续 Snapshot 不得再覆盖该字段。
3. 草稿 scope 必须由 `sessionId ?? draftScopeIdOverride ?? "__draft__"` 统一解析；
   配置 owner、正文持久化、首条提升（promote）和清理使用同一个 scope。
4. 设备对话、新任务、会话面板各自独立；`__draft__` 只属于主新任务面板。
5. “带初始 Prompt 新建任务”只替换正文相关字段，不删除已有的
   `mode`/`planEnabled`/`modelSelection`/授权标记。
6. 无法被当前 ModelSelectionView 解析的选择保持原样；界面可以显示为未解析/要求
   重选，但不能把草稿清空。

## 事件顺序

```text
用户选模型
  → updateDraftConfig 写入 modelSelection（草稿 owner）
  → 已有会话: switchModelConfig (CAS, Host)
  → 会话 Snapshot 到达: 只补缺失的 mode/plan，不覆盖 modelSelection
  → 提交: 冻结 draft 的 modelSelection 作为本次 intent
```

## 兼容与迁移

- 不改变 localStorage key 与草稿 JSON 版本。
- 旧草稿没有 `mode` 但有 `modelSelection` 时，按规则 1 保留原模型，只补
  `mode`/`planEnabled`。
- 旧设备对话草稿可能写在 `__draft__` 下，升级后不会再被设备面板读取；属于一次性
  可见变化，不回填、不迁移，避免把主新任务草稿误判为设备对话内容。

## 验收

1. 打开一个没有本地草稿的会话，在 Snapshot 到达前选中模型；Snapshot 到达后，
   草稿与选择器仍显示新模型，下一次提交携带新模型。
2. 设备对话（尚无 session）选择模型不改变主新任务面板的模型；主面板发送后
   设备对话草稿不丢失。
3. 带 `initialPrompt` 新建任务时，已有的模型/模式意图保留，正文被替换。
4. `pnpm --filter @mode/ui test`、`pnpm typecheck`、`pnpm lint` 通过。
