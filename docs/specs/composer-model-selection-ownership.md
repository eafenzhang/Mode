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
7. 用户显式选模（模型菜单点击）后，除写草稿外还要把完整选择写回 Host 的
   `defaultModelSelection`（Personal 配置文件字段）；这一步是跨 localStorage 的
   持久兜底，写失败只记日志，不阻断选择。
8. **会话真值回填（2026-10-25，重启丢模型修复）**：已有会话（sessionId 非空）的草稿
   `modelSelection` 为空（undefined——没有显式值，也没有被初始化种子填充）而会话
   Snapshot `config.modelSelection` 存在时，把会话真值回填进草稿。判据与边界：
   - 草稿持久化是 best-effort localStorage（配额/键丢失/历史版本都可能丢
     modelSelection），agent 会话配置才是既有会话的权威事实；没有这条回填，
     重启后选择器永远回落默认模型（2026-10-25 用户实测）。
   - 只填**空**草稿：草稿已有任何 modelSelection（用户显式选择，含与快照不同）
     时不碰——规则 2 的「不得覆盖显式值」不变。
   - 已有会话选模即时发 `switchModelConfig`，agent 侧事实随之更新；因此即使草稿
     持久化失败，重启也能经本条从 agent 真值还原。

## 事件顺序

```text
用户选模型
  → updateDraftConfig 写入 modelSelection（草稿 owner）
  → 已有会话: switchModelConfig (CAS, Host)
  → 会话 Snapshot 到达: 只补缺失的 mode/plan，不覆盖 modelSelection
  → 提交: 冻结 draft 的 modelSelection 作为本次 intent
重启/草稿丢失
  → 草稿 modelSelection 为空 + 会话 Snapshot config.modelSelection 存在
  → 回填草稿（规则 8）；已有显式值不碰
```

## 兼容与迁移

- 当前 localStorage key 与草稿 JSON 版本不变（`mode-v4-composer-drafts:v1:<key>` /
  `mode-model-selection-recent-v1:<key>`，version 1）。
- P2 改名（17e0dd22）曾把 `zcode-` 前缀换成 `mode-` 但没有迁移旧键：升级后旧键里的
  草稿与最近提交选择读不到，重启表现为丢模型/丢正文。读取侧在新键为空时做一次性
  搬迁：旧键有数据 → 写入同后缀新键并删除旧键；搬迁失败（配额/异常）只降级为不
  迁移，不影响输入。写入侧永远只写新键。
- 旧草稿没有 `mode` 但有 `modelSelection` 时，按规则 1 保留原模型，只补
  `mode`/`planEnabled`。
- 旧设备对话草稿可能写在 `__draft__` 下，升级后不会再被设备面板读取；属于一次性
  可见变化，不回填、不迁移，避免把主新任务草稿误判为设备对话内容。
- 显式选模后 Renderer 通过 `IModelSelectionService.saveConfiguredDefault` 把完整
  选择（含 reasoningLevel）写回 Personal 配置的 `defaultModelSelection`，与 CLI
  `/model` 同语义。该方法是可选 RPC 方法：旧 Host 或未装配写入的 Environment 没有
  它，调用方必须容错（失败只记 warn，不影响本次选择）。新任务初始化读到有效
  `defaultModelSelection` 时优先于 registry 首选；失效默认仍按 Registry 顺序回退。

## 验收

1. 打开一个没有本地草稿的会话，在 Snapshot 到达前选中模型；Snapshot 到达后，
   草稿与选择器仍显示新模型，下一次提交携带新模型。
2. 设备对话（尚无 session）选择模型不改变主新任务面板的模型；主面板发送后
   设备对话草稿不丢失。
3. 带 `initialPrompt` 新建任务时，已有的模型/模式意图保留，正文被替换。
4. localStorage 里只有旧 `zcode-` 前缀键时，打开工作区后草稿与最近选择仍可读，
   旧键被搬迁到 `mode-` 前缀并删除。
5. 显式选模后 `provider_config.json` 的 `defaultModelSelection` 更新为所选完整
   清单；删除 localStorage 后新建任务草稿仍恢复到该模型。
6. `pnpm --filter @mode/ui test`、`pnpm typecheck`、`pnpm lint` 通过。
