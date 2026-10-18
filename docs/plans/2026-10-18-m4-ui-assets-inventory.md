# M4 UI 资产盘点：来源、台账号与替换建议（2026-10-18）

> 输入：`packages/ui/src/assets` 的文件清单、重合度报告（`.plan/overlap.json`）、
> `third-party/copied-components.json`。只做来源分类，不判断商标/再分发条款的法律结论。

## 总量与分布

`packages/ui/src/assets` 共 **324** 个文件：

| 目录 | 文件数 | 与上游 identical | 说明 |
| --- | --- | --- | --- |
| `plugin-icons/` | 284 | 6 | 官方插件目录的品牌图标；其中 278 个已在台账标 `reviewRequired` |
| `provider-icons/` | 25 | 21 | 模型/供应商品牌图标；**台账没有条目** |
| `channel-icons/` | 7 | 7 | 飞书/钉钉/企微/微信/Telegram/Discord 品牌图标 + `index.ts` |
| `cli-icons/` | 3 | 3 | GLM 品牌图标（深浅色两版） |
| `document-skill-icons/` | 3 | 3 | 文档/PDF/表格技能图标 |
| `notification-sounds/` | 1 | 1 | 任务完成提示音（mp3） |
| 根目录 | 1 | 1 | `chevron-down-md.svg` |

## 来源分类

**A. 已在第三方台账、但条款未落实（278 + 25）**

- `plugin-icons/mode-plugins-official`：`copied-components.json` 的
  「Official plugin marketplace icons」条目写明 278 个第三方插件品牌图标
  **没有逐图标许可记录**，`reviewRequired` 要求「核实条款或改为按需获取/通用图标」；
- `provider-icons/`：25 个供应商图标（其中 21 个与上游 identical）**没有台账条目**，
  属来源缺口——它们同样是第三方商标/品牌资产，需要在 `copied-components.json`
  登记来源与再分发依据，或替换。逐图标来源与待补台账见
  `2026-10-18-m4-provider-icons-inventory.md`（17 个已有来源映射、7 个未记录）。

**B. 与上游逐字节一致、需要重画或删除（42）**

`channel-icons`（7）、`cli-icons`（3）、`document-skill-icons`（3）、`notification-sounds`（1）、
根目录 chevron（1）、`plugin-icons`（6）、`provider-icons`（21）。
其中通知音与 chevron 没有品牌属性，可以直接重做；其余是品牌资产，处理方式取决于 A 的决策。

**C. 自研资产**

`plugin-icons` 的 278 个台账条目中，有一部分是 Mode 自己的插件图标（第一方），
它们不需要第三方许可；P1.7 需要把「第一方图标」与「第三方品牌图标」拆开登记，
不能整目录删除。

## 对 P1.7 的建议

1. **先分类再动手**：把 `plugin-icons` 拆成「第一方插件图标（保留/重画）」与
   「第三方品牌图标（待条款决策）」两份清单；`provider-icons` 同样先补台账。
2. **非品牌资产直接重做**：`chevron-down-md.svg`、`task-notification-pop.mp3`
   与上游一致但无品牌属性，按 `DESIGN.md` 重画/重录即可归零。
3. **品牌资产三选一**（需要产品/法务拍板）：
   - (a) 保留并补齐逐来源许可/再分发依据；
   - (b) 改为运行时按需获取（离线时显示通用图标）；
   - (c) 统一换成通用/自绘图标。
   `plugin-icons` 的台账已经明确把 (a) 与 (b) 列为候选。
4. **不要先删 `plugin-icons`**：`packages/ui/src/settings/modeOfficialPluginIcons.generated.ts`
   按名称映射这些图标，删除前必须同步更新生成器与商店/插件列表的回退图标。

## 验收

- `packages/ui/src/assets` 在重合度报告里 identical/normalized 归零（C 类自研资产除外）；
- `copied-components.json` 覆盖所有第三方品牌资产，或它们在 UI 中已被通用图标替代；
- `plugin-icons` 的第一方/第三方拆分有清单，商店卡片在缺图时走通用回退。

## 待裁决

品牌图标策略（保留并补许可 / 按需获取 / 通用图标）是产品与法务决策；
本文件只给出分类与依赖关系，不替用户选择。
