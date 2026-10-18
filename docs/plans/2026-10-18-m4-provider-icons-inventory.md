# M4 UI 资产：provider-icons 逐图标来源清单（2026-10-18）

> 输入：`packages/ui/src/assets/provider-icons/` 的 25 个文件、
> `model-provider-logo-sources.json`（仓库内已有的来源映射）、重合度报告、`copied-components.json`。
> 只做来源与台账核对，不判断商标/再分发条款。

## 现状

- 目录内 **24 个图标 + 1 个来源映射**（`model-provider-logo-sources.json`，ours-only）；
- 重合度：**21 个与上游 identical**、1 个 distinct（来源映射自身）；PNG 行数为 0（二进制）；
- `copied-components.json` **没有** provider-icons 条目——这是台账缺口；
- 仓库里已有一份**逐图标来源映射**（2026-10-13 复核），记录了 17 个图标（含 dark 变体）的
  brand / providerIds / sourceUrl / sourceOwner / normalization。

## 已被来源映射覆盖的 17 个文件

| 品牌 | 文件 |
| --- | --- |
| OpenRouter | `model-provider-openrouter-light.svg` / `-dark.svg` |
| OpenCode | `model-provider-opencode-light.svg` / `-dark.svg` |
| Start Plan | `model-provider-start-plan.png` |
| Z.AI | `model-provider-zai-app.png` |
| BigModel | `logo-bigmodel.svg` |
| Moonshot Kimi | `model-provider-moonshot-kimi.png` |
| MiniMax | `model-provider-minimax.png` |
| DeepSeek | `model-provider-deepseek.png` |
| Alibaba Model Studio | `model-provider-alibaba-cloud.png` |
| Xiaomi MiMo | `model-provider-xiaomi-mimo.png` |
| OpenAI | `model-provider-openai.png` |
| Anthropic | `model-provider-anthropic.png` |
| xAI | `model-provider-xai.png` |
| OrcaRouter | `model-provider-orcarouter.png` |
| TokenDance | `model-provider-tokendance.svg` |

来源类型：官方品牌页/GitHub 组织头像/官方 CDN、用户提供（Start Plan）、仓库内已有资产
（Z.AI）。这些**有来源记录**，但没有进入 `copied-components.json` 的台账链条。

## 未被来源映射覆盖的 7 个文件

| 文件 | 与上游 | 说明 |
| --- | --- | --- |
| `logo-anthropic.svg` | identical | 上游遗留图标，来源未记录 |
| `logo-moonshoot.svg` | identical | 同上 |
| `logo-open-audit.svg` | identical | 同上 |
| `logo-openrouter.svg` | identical | 同上（与新版 light/dark 并存） |
| `logo-zai.svg` | identical | 同上 |
| `logo-zai-square.svg` | identical | 同上 |
| `model-provider-zai.png` | identical | 上游遗留；与来源映射里的 `model-provider-zai-app.png` 不是同一文件 |

这 7 个文件属于「上游衍生 + 第三方品牌」双重属性：既计入 M4 重合度，也没有逐图标来源。
P1.7 应优先处理——重画、替换为通用图标，或补来源记录。

## 建议的台账条目（待 T3 批次一起提）

`copied-components.json` 增加一条：

```json
{
  "id": "Provider brand icons",
  "source": "Per-icon provenance in packages/ui/src/assets/provider-icons/model-provider-logo-sources.json (17 files); the 7 legacy logo-* / model-provider-zai.png files are upstream-derived with no recorded per-icon terms",
  "importRevision": null,
  "provenance": "PNG/SVG copies of third-party model-provider brand icons bundled for offline display",
  "roots": ["packages/ui/src/assets/provider-icons"],
  "scope": "Bundled provider logos used by the provider/model pickers; per-brand redistribution terms are not pinned for the 7 legacy files.",
  "license": "not established",
  "reviewRequired": "Verify per-brand terms or replace with generic icons; 7 upstream-derived files have no source record."
}
```

注意：`copied-components.json` 是 notices 生成器的输入，改它会改变
`THIRD-PARTY-NOTICES.md` 与 `inventory.inputs` 的哈希。**不要单独提交**，应与下一批
台账/notice 重生成（T3）一起走 Linux CI，避免 licenses-notices 持续红灯。

## 对 P1.7 的处置顺序

1. 先用来源映射补齐 17 个已记录图标的台账信息（条目如上）；
2. 对 7 个未覆盖文件二选一：补来源记录，或重画/换通用图标（同时降低重合度）；
3. 品牌图标策略（保留并补许可 / 按需获取 / 通用图标）仍是产品与法务决策，
   见 `2026-10-18-m4-ui-assets-inventory.md`。
