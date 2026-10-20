# 工具卡文件入口的右侧预览目标

状态：已实现（2026-10-20）。适用范围：Edit / Write 工具卡、Changes 分组卡。

## 规则

- 工具卡**折叠态的内联预览**继续显示 diff（`buildEditCodeViewerSource`），用于说明本次改了什么。
- 点击工具卡里的文件 chip / 文件入口时，右侧预览打开**文件源码**（`type: "file"`），
  而不是再次打开同一份 diff。用户点文件名的意图是看"现在文件是什么样"，不是复习绿色新增行。
- diff 仍可从工具卡内联预览和 Git/变更摘要入口打开；不在文件点击路径上重复展示。
- 右侧面板只消费 source，不自己判断工具类型；source 的选择归工具卡 renderer。

## 所有权与边界

| 事实 | 所有者 |
| --- | --- |
| 工具卡点击打开哪种 source | `ToolCallBlocks/renderers/edit.tsx`、`changes-group.tsx` |
| diff 的内联预览 | `EditInlineDiffContent`（仍由 `buildEditCodeViewerSource` 生成 patch） |
| 右侧 tab 的复用与激活 | `openCodeViewerSidePane`（按 workspace + sourceKey） |

`buildEditFileViewerSource` 只返回 `file` source，不改变 patch 生成、内联 diff 或 Git diff 的语义。

## 验收

- `buildEditFileViewerSource` 对带 patch 的 summary 仍返回 `type: "file"`。
- Edit / Write 工具卡与 Changes 分组卡的文件点击都使用该函数。
- 内联预览仍使用 `buildEditCodeViewerSource` 生成的 patch。
