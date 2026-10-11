# 远端工作区对话双向附件 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让桌面窗口的远端工作区对话（SSH/WSL/Docker/LAN）支持附件双向收发：LAN 解禁发送、任意 MIME 历史附件回读下载、从远端工作区选文件拉回。

**Architecture:** 发向复用两条既有通道——有 backend 的传输走 `IPromptAttachmentTransferService.stage` 零拷贝暂存（LAN 用 `attachmentBeginV4/ChunkV4/CommitV4` 字节通道实现同一契约），收向把网关的行授权从「媒体限定」拆为「权威行任意 MIME」并接 `platform.saveFile` 下载。协议限额一律不动（上行 20 MiB、读 20/30 MiB）。

**Tech Stack:** TypeScript、node:test（desktop 用 `.test.mts` + `--import tsx` 夹具模式）、tsx --test（ui/bootstrap）、zod 协议 schema、Electron host/renderer。

**Spec:** `docs/specs/remote-workspace-bidirectional-attachments.md`（本计划的验收与不变量以它为准；实现前先通读）。

## Global Constraints

- **限额零改动**：`PROTOCOL_V4_LIMITS.attachmentMaxBytes = 20 MiB`、`attachmentChunkMaxBytes = 512 KiB`、`attachmentUploadMaxChunks = 64`、读容量非视频 20 MiB / 视频 30 MiB，全部保持；任何「提高上限」的改动都不属于本计划（见 spec 附录）。
- **不变量 1**：主机本地路径永不抵达远端 agent（stage / materialize 排序判定兜底）。
- **不变量 2**：回读只信权威行（`userInput`/`artifact` 行 ref 精确匹配，跨 session 拒绝）——Task 1 是安全核心。
- **i18n**：新增文案 key 必须同时写 `packages/ui/src/i18n/locales/zh-CN.ts`、`en-US.ts`、`fa.ts` 三份（沿用既有 flat key 格式）。
- **语言政策**（`docs/specs/language-policy.md`）：新文件禁 `.js/.mjs/.cjs`；desktop 新测试用 `.test.mts`（Node 24 type-stripping，`node --test` 可直接跑），desktop 源码导入测试走「tsx 夹具进程」模式（先例：`packages/desktop/tests/lan-attach-initialize.test.mjs`）。
- **host 错误文案用中文**（沿用 `host/index.ts` 现行模式）；renderer 可见 UI 文案走 i18n key。
- **提交纪律**：每个 Task 结束本地 commit，不 push；保留工作区无关改动。
- **最终验证**：`pnpm typecheck` 与 `pnpm lint` 必须实际执行并如实报告；另跑 `pnpm architecture:check --changed`。
- workspace 包（`@mode/shared` 等）测试依赖其 `dist` 产物已构建（日常开发态已满足；缺失时先 `pnpm --filter @mode/shared build`）。

## 文件结构总表

| 动作 | 文件 | 职责 |
| --- | --- | --- |
| Create | `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/attachment-row-auth.ts` | 附件行授权纯函数（任意 MIME / mediaOnly 双语义） |
| Create | `apps/mode-cli/packages/bootstrap/test/attachment-row-auth.test.mts` | 授权矩阵测试 |
| Modify | `apps/mode-cli/packages/bootstrap/package.json` | 加 `test` 脚本 |
| Modify | `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/v4-gateway.ts` | attachmentRead/attachmentPreviewSource 接纯函数；删 readAttachmentPayload 媒体硬卡 |
| Create | `packages/ui/src/lib/attachmentDownload.ts` | 下载编排 + 错误分类 + 读参构造（纯） |
| Create | `packages/ui/src/hooks/useAttachmentDownload.ts` | react hook：toast/进度/平台 saveFile 接线 |
| Create | `packages/ui/test/attachmentDownload.test.ts` | 下载纯逻辑测试 |
| Modify | `packages/ui/src/v4/ConversationRowView.tsx` | 文件 pill 下载按钮、媒体预览 onDownload、ArtifactRowView 下载 |
| Modify | `packages/ui/src/ChatMediaAttachmentPreviewDialog.tsx` | 下载按钮 |
| Modify | `packages/shared/src/test-ids.ts` | 新 TID |
| Modify | `packages/desktop/src/host/remotePromptAttachments.ts` | SSH materialize 排序判定；导出 `isPathInRoot`/`replaceContentPaths` |
| Create | `packages/desktop/src/host/lanPromptAttachments.ts` | LAN stage（chunk 通道上传）+ LAN materialize |
| Create | `packages/desktop/tests/fixtures/remote-prompt-attachments-order.ts` + `packages/desktop/tests/remote-prompt-attachments-order.test.mts` | 排序判定夹具 |
| Create | `packages/desktop/tests/fixtures/lan-prompt-attachments.ts` + `packages/desktop/tests/lan-prompt-attachments.test.mts` | LAN stage 夹具 |
| Create | `packages/desktop/tests/fixtures/lan-materialize.ts` + `packages/desktop/tests/lan-materialize.test.mts` | LAN materialize 夹具 |
| Modify | `packages/desktop/src/host/index.ts` | LAN 句柄 stub 换真实现 |
| Modify | `packages/ui/src/lib/chatAttachments.ts` | `workspacePick` 字段 + 构造器 |
| Modify | `packages/ui/src/v4/composer/useComposerAttachments.ts` | 转移模式判定 + `addWorkspacePickAttachments` |
| Create | `packages/ui/test/composerWorkspacePickAttachment.test.ts` | 模型测试 |
| Modify | `packages/ui/src/DirectoryBrowser.tsx` | file 模式（多选）+ `initialPath` |
| Create | `packages/ui/test/directoryBrowserFileMode.test.ts` | 纯过滤测试 |
| Modify | `packages/ui/src/prompt-editor/ChatPromptActionMenu.tsx`、`ChatPromptEditor.tsx`、`v4/ConversationComposer.tsx` | `attachmentActions` 数组链 + 选文件入口接线 |
| Modify | `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts` | 全部新文案 |

---

### Task 1: 网关附件行授权（收向安全核心）

**Files:**
- Create: `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/attachment-row-auth.ts`
- Create: `apps/mode-cli/packages/bootstrap/test/attachment-row-auth.test.mts`
- Modify: `apps/mode-cli/packages/bootstrap/package.json`（scripts 加 `"test": "tsx --test test/*.test.mts"`）
- Modify: `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/v4-gateway.ts`（`resolveReadableMediaAttachment` 方法、`attachmentRead`、`attachmentPreviewSource`、`readAttachmentPayload`）

**Interfaces:**
- Consumes: `ConversationRow` / `AttachmentRef`（`@mode/shared/mode-protocol-v4`）、`extractMarkdownArtifactImageRefs`（`@mode/shared`）
- Produces: `authorizeConversationAttachment(rows, params): AuthorizedConversationAttachment | null`，`params = { sessionId, ref, target?, attachmentIndex?, mediaOnly: boolean }`；`result = { attachment: AttachmentRef; attachmentIndex?: number; sourceRowId?: number }`。Task 3/4 的下载读参构造以 `mediaOnly:false` 授权面为前提。

- [ ] **Step 1: 写失败测试**

创建 `apps/mode-cli/packages/bootstrap/test/attachment-row-auth.test.mts`：

```ts
/**
 * 附件行授权矩阵（spec: remote-workspace-bidirectional-attachments 收向不变量 2）。
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { authorizeConversationAttachment } from "../src/mode-protocol-v4/attachment-row-auth.js";
import type { ConversationRow } from "@mode/shared/mode-protocol-v4";

// 测试夹具只依赖授权读取的字段；zod 全量必填字段与授权无关，用 unknown 断言收窄。
function row(partial: Record<string, unknown>): ConversationRow {
  return {
    rowId: 1,
    turnId: "turn-1",
    entityId: "ent-1",
    createdAt: 0,
    createdAtSeq: 0,
    ...partial,
  } as unknown as ConversationRow;
}

const pdfAttachment = {
  ref: "mode-artifact://s1/a1",
  fileName: "a.pdf",
  mime: "application/pdf",
  bytes: 100,
};
const textAttachment = {
  ref: "/home/u/notes.txt",
  fileName: "notes.txt",
  mime: "text/plain",
  bytes: 10,
  previewRef: "preview/notes.txt",
};

test("target+index：userInput 行 ref 精确匹配，任意 MIME 授权（mediaOnly:false）", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "/home/u/notes.txt",
    target: { rowId: 1, entityId: "ent-1" },
    attachmentIndex: 0,
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachmentIndex, 0);
  assert.equal(result.sourceRowId, 1);
  assert.equal(result.attachment.fileName, "notes.txt");
});

test("target+index：mediaOnly:true 时非媒体拒绝", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "/home/u/notes.txt",
    target: { rowId: 1, entityId: "ent-1" },
    attachmentIndex: 0,
    mediaOnly: true,
  });
  assert.equal(result, null);
});

test("target+index：ref 不匹配 / 跨行 entityId 拒绝", () => {
  const rows = [row({ kind: "userInput", attachments: [pdfAttachment] })];
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://other/a1",
      target: { rowId: 1, entityId: "ent-1" },
      attachmentIndex: 0,
      mediaOnly: false,
    }),
    null,
  );
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://s1/a1",
      target: { rowId: 2, entityId: "ent-1" },
      attachmentIndex: 0,
      mediaOnly: false,
    }),
    null,
  );
});

test("无 target 扫描：userInput 按 previewRef ?? ref 授权（保持旧不对称语义）", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "preview/notes.txt",
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachmentIndex, undefined);
  assert.equal(result.sourceRowId, undefined);
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "preview/other.txt",
      mediaOnly: false,
    }),
    null,
  );
});

test("artifact 行：按 ref 授权，行字段合成 AttachmentRef", () => {
  const rows = [
    row({
      kind: "artifact",
      ref: "mode-artifact://s1/run/report.pdf",
      displayName: "report.pdf",
      mimeType: "application/pdf",
      sizeBytes: 2048,
    }),
  ];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/run/report.pdf",
    target: { rowId: 1, entityId: "ent-1" },
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachment.fileName, "report.pdf");
  assert.equal(result.attachment.mime, "application/pdf");
  assert.equal(result.attachment.bytes, 2048);
  assert.equal(result.attachmentIndex, undefined);
  // 媒体 gate：非媒体 artifact 在 mediaOnly 下拒绝。
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://s1/run/report.pdf",
      mediaOnly: true,
    }),
    null,
  );
});

test("artifact 行：无 target 扫描同样授权", () => {
  const rows = [
    row({
      kind: "artifact",
      ref: "mode-artifact://s1/img.png",
      displayName: "img.png",
      mimeType: "image/png",
      sizeBytes: 5,
    }),
  ];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/img.png",
    mediaOnly: true,
  });
  assert.ok(result);
});

test("assistantText markdown 图片：session 归属精确匹配才放行", () => {
  const rows = [row({ kind: "assistantText", text: "![x](mode-artifact://s1/img.png)" })];
  const ok = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/img.png",
    mediaOnly: false,
  });
  assert.ok(ok);
  assert.equal(ok.sourceRowId, undefined);
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s2",
      ref: "mode-artifact://s1/img.png",
      mediaOnly: false,
    }),
    null,
  );
});

test("target 无 index：artifact 行按行身份授权，非匹配 ref 拒绝", () => {
  const artifactRow = row({
    kind: "artifact",
    ref: "mode-artifact://s1/r.bin",
    displayName: "r.bin",
    mimeType: "application/octet-stream",
    sizeBytes: 1,
  });
  const artifactAuth = authorizeConversationAttachment([artifactRow], {
    sessionId: "s1",
    ref: "mode-artifact://s1/r.bin",
    target: { rowId: 1, entityId: "ent-1" },
    mediaOnly: false,
  });
  assert.ok(artifactAuth);
  assert.equal(artifactAuth.attachmentIndex, undefined);
  assert.equal(
    authorizeConversationAttachment([artifactRow], {
      sessionId: "s1",
      ref: "mode-artifact://s1/other",
      target: { rowId: 1, entityId: "ent-1" },
      mediaOnly: false,
    }),
    null,
  );
});
```

- [ ] **Step 2: 加 test 脚本并运行，确认失败**

`apps/mode-cli/packages/bootstrap/package.json` 的 `scripts` 改为：

```json
"scripts": {
  "build": "tsc",
  "clean": "node ../../scripts/clean-dist.mjs",
  "typecheck": "tsc --noEmit",
  "test": "tsx --test test/*.test.mts",
  "lint": "oxlint src",
  "lint:fix": "oxlint --write src"
}
```

Run: `pnpm --filter @mode/bootstrap test`
Expected: FAIL —— 模块 `../src/mode-protocol-v4/attachment-row-auth.js` 不存在。

- [ ] **Step 3: 实现授权纯函数**

创建 `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/attachment-row-auth.ts`：

```ts
// 附件读授权（v4-gateway 附件读面唯一裁决点）。
// 规则：ref 必须命中当前 session 权威投影中的行——userInput 行按 ref/previewRef
// 精确匹配，artifact 行按 ref 精确匹配，assistant markdown 只认本 session 的
// mode-artifact URI；跨 session、任意路径一律 null。mediaOnly 只是媒体 gate
// （attachmentPreviewSource 旧语义），不参与授权本身。
import { extractMarkdownArtifactImageRefs } from "@mode/shared";
import type { AttachmentRef, ConversationRow } from "@mode/shared/mode-protocol-v4";

export interface ConversationRowAuthTarget {
  rowId: number;
  entityId: string;
}

export interface AuthorizedConversationAttachment {
  attachment: AttachmentRef;
  /** 仅 userInput target+index 授权携带（与旧实现的 messageId 语义一致）。 */
  attachmentIndex?: number;
  sourceRowId?: number;
}

export interface AuthorizeConversationAttachmentParams {
  sessionId: string;
  ref: string;
  target?: ConversationRowAuthTarget;
  attachmentIndex?: number;
  /** true = 只放行 image/video/pdf（previewSource 语义）；false = 任意 MIME（read 语义）。 */
  mediaOnly: boolean;
}

export function artifactRefBelongsToSession(ref: string, sessionId: string): boolean {
  return ref.startsWith(`mode-artifact://${encodeURIComponent(sessionId)}/`);
}

function isPreviewableMime(mime: string): boolean {
  const normalized = mime.split(";", 1)[0]?.trim().toLowerCase() ?? "";
  return (
    normalized.startsWith("image/") ||
    normalized.startsWith("video/") ||
    normalized === "application/pdf"
  );
}

function matchesRef(attachment: AttachmentRef, ref: string): boolean {
  return attachment.ref === ref || attachment.previewRef === ref;
}

function artifactRowAttachmentRef(
  row: Extract<ConversationRow, { kind: "artifact" }>,
): AttachmentRef {
  return {
    ref: row.ref,
    fileName: row.displayName,
    mime: row.mimeType,
    bytes: row.sizeBytes,
  };
}

export function authorizeConversationAttachment(
  rows: readonly ConversationRow[],
  params: AuthorizeConversationAttachmentParams,
): AuthorizedConversationAttachment | null {
  const { sessionId, ref, target, attachmentIndex, mediaOnly } = params;
  const allowed = (mime: string) => !mediaOnly || isPreviewableMime(mime);

  if (target && attachmentIndex !== undefined) {
    const row = rows.find(
      (candidate) => candidate.rowId === target.rowId && candidate.entityId === target.entityId,
    );
    if (row?.kind !== "userInput") return null;
    const attachment = row.attachments?.[attachmentIndex];
    if (!attachment || !matchesRef(attachment, ref) || !allowed(attachment.mime)) return null;
    return { attachment, attachmentIndex, sourceRowId: row.rowId };
  }

  if (target) {
    // 带 target 无 index：只按行身份授权 artifact（ref 精确匹配）；不误授权该行其他附件。
    const row = rows.find(
      (candidate) => candidate.rowId === target.rowId && candidate.entityId === target.entityId,
    );
    if (row?.kind === "artifact" && row.ref === ref && allowed(row.mimeType)) {
      return { attachment: artifactRowAttachmentRef(row) };
    }
    if (row?.kind === "artifact") return null;
    // userInput 行缺 index 无法定位附件，退回扫描（与旧实现「target 未命中即全量扫描」对齐）。
  }

  for (const row of rows) {
    if (row.kind === "userInput") {
      for (const attachment of row.attachments ?? []) {
        if (!allowed(attachment.mime)) continue;
        if ((attachment.previewRef ?? attachment.ref) === ref) {
          // 旧扫描分支不携带 index/messageId（缓存 key 走 legacy），保持不变。
          return { attachment };
        }
      }
      continue;
    }
    if (row.kind === "artifact" && row.ref === ref && allowed(row.mimeType)) {
      return { attachment: artifactRowAttachmentRef(row) };
    }
    if (
      row.kind === "assistantText" &&
      artifactRefBelongsToSession(ref, sessionId) &&
      extractMarkdownArtifactImageRefs(row.text).includes(ref)
    ) {
      return { attachment: { ref, fileName: "assistant-image", mime: "image/*", bytes: 0 } };
    }
  }
  return null;
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `pnpm --filter @mode/bootstrap test`
Expected: PASS（8 个 test 全绿）

- [ ] **Step 5: 接线 gateway（TDD 后的生产替换）**

`v4-gateway.ts` 修改：

1. 顶部 import 增加：

```ts
import {
  authorizeConversationAttachment,
  type AuthorizedConversationAttachment,
} from "./attachment-row-auth.js";
```

2. 删除私有方法 `resolveReadableMediaAttachment`（约 2151-2216 行，含其内联的 `isPreviewable`）与文件内私有函数 `artifactRefBelongsToSession`（约 530-532 行，已移入新模块）；`extractMarkdownArtifactImageRefs` 的 import 若无其他使用点一并删除。删除前 grep 确认全部调用点已改走新函数。

3. `attachmentRead`（1969 行起）授权段替换为：

```ts
    const resolution = authorizeConversationAttachment(publisher.getSnapshot().rows.window, {
      sessionId: params.sessionId,
      ref: params.ref,
      ...(params.target ? { target: params.target } : {}),
      ...(params.attachmentIndex !== undefined
        ? { attachmentIndex: params.attachmentIndex }
        : {}),
      mediaOnly: false,
    });
    if (!resolution) {
      // renderer 传来的 ref 不能直接成为文件路径；必须先由当前 session
      // 的权威 user/artifact 行证明归属，避免跨 session 或任意路径读取。
      throw new Error("fault.attachment.previewRefNotAuthorized");
    }
    const messageId =
      resolution.sourceRowId !== undefined
        ? publisher.getMessageIdForRow(resolution.sourceRowId) ?? undefined
        : undefined;
```

其后对 `readAttachmentPayload` 的调用参数保持不变（`resolution.attachment.mime`、`messageId`、`resolution.attachmentIndex`）。

4. `attachmentPreviewSource`（2115 行起）中 `resolveReadableMediaAttachment(...)` 调用替换为：

```ts
    const resolution = authorizeConversationAttachment(publisher.getSnapshot().rows.window, {
      sessionId: params.sessionId,
      ref: params.ref,
      ...(params.target ? { target: params.target } : {}),
      ...(params.attachmentIndex !== undefined
        ? { attachmentIndex: params.attachmentIndex }
        : {}),
      mediaOnly: true,
    });
    if (!resolution) {
      throw new Error("fault.attachment.previewRefNotAuthorized");
    }
```

后续 video / `resolveSessionAttachmentPreviewSource` 分支不动。

5. `readAttachmentPayload`（约 2247-2263 行）删除媒体硬卡分支（`previewNotMedia` 抛错段），**保留** `maxBytes` 计算原样（`allowGeneric ? attachmentPreviewMaxBytes : (video ? attachmentPreviewMaxBytes : attachmentMaxBytes)`）。`MODE_ATTACHMENT_FAULT_CODES.previewNotMedia` 常量保留（旧版本对端兼容文案映射，见 Task 3）。

- [ ] **Step 6: 全量验证并提交**

Run: `pnpm --filter @mode/bootstrap test && pnpm --filter @mode/bootstrap typecheck && pnpm --filter @mode/bootstrap lint`
Expected: 全部 PASS（若报未使用 import 等问题，按报错修整后重跑）

```bash
git add apps/mode-cli/packages/bootstrap/
git commit -m "feat(attachments): 网关附件行授权放开 MIME 并覆盖 artifact 行"
```

---

### Task 2: UI 下载纯逻辑（编排、错误分类、读参构造）

**Files:**
- Create: `packages/ui/src/lib/attachmentDownload.ts`
- Create: `packages/ui/test/attachmentDownload.test.ts`

**Interfaces:**
- Consumes: `AttachmentRef`、`ConversationRowTarget`（`@mode/shared/mode-protocol-v4`）、`IPlatformService["saveFile"]` 载荷形状（`packages/shared/src/platform.ts:256-273`）
- Produces: `downloadAttachment(deps, params)`、`buildAttachmentDownloadReadParams(input)`、`classifyAttachmentDownloadError(error)`、`suggestedAttachmentDownloadName(fileName)`、类型 `AttachmentDownloadReadParams` / `AttachmentDownloadOutcome`。Task 3 的 `useAttachmentDownload` hook 全部消费这些导出。

- [ ] **Step 1: 写失败测试**

创建 `packages/ui/test/attachmentDownload.test.ts`：

```ts
/** 附件下载纯逻辑（spec: remote-workspace-bidirectional-attachments 收向下载）。
 * 运行：pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildAttachmentDownloadReadParams,
  classifyAttachmentDownloadError,
  downloadAttachment,
  suggestedAttachmentDownloadName,
} from "../src/lib/attachmentDownload.js";

test("classifyAttachmentDownloadError：稳定码 → 分类", () => {
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewTooLarge")),
    "tooLarge",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewRefNotAuthorized")),
    "unauthorized",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewNotMedia")),
    "legacyPeer",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.readUnsupported")),
    "unsupported",
  );
  assert.equal(classifyAttachmentDownloadError(new Error("socket hang up")), "unknown");
});

test("suggestedAttachmentDownloadName：去路径分隔符与控制字符，空值回退", () => {
  assert.equal(suggestedAttachmentDownloadName("报表 2024.pdf"), "报表 2024.pdf");
  assert.equal(suggestedAttachmentDownloadName("a/b\\c.txt"), "abc.txt");
  assert.equal(suggestedAttachmentDownloadName(""), "attachment");
});

test("buildAttachmentDownloadReadParams：previewRef 优先 + entityId 才带 target/index", () => {
  const attachment = {
    ref: "/remote/notes.txt",
    previewRef: "preview/notes.txt",
    fileName: "notes.txt",
    mime: "text/plain",
    bytes: 1,
  };
  const withEntity = buildAttachmentDownloadReadParams({
    attachment,
    sessionId: "s1",
    rowId: 7,
    entityId: "ent-7",
    attachmentIndices: [3],
    index: 0,
  });
  assert.equal(withEntity.ref, "preview/notes.txt");
  assert.deepEqual(withEntity.target, { rowId: 7, entityId: "ent-7" });
  assert.equal(withEntity.attachmentIndex, 3);

  const withoutEntity = buildAttachmentDownloadReadParams({
    attachment,
    sessionId: "s1",
    rowId: 7,
    index: 0,
  });
  assert.equal(withoutEntity.target, undefined);
  assert.equal(withoutEntity.attachmentIndex, undefined);
});

test("buildAttachmentDownloadReadParams：artifact 行（includeAttachmentIndex:false）只带 target", () => {
  const params = buildAttachmentDownloadReadParams({
    attachment: {
      ref: "mode-artifact://s1/r.pdf",
      fileName: "r.pdf",
      mime: "application/pdf",
      bytes: 9,
    },
    sessionId: "s1",
    rowId: 12,
    entityId: "ent-12",
    index: 0,
    includeAttachmentIndex: false,
  });
  assert.deepEqual(params.target, { rowId: 12, entityId: "ent-12" });
  assert.equal(params.attachmentIndex, undefined);
});

test("downloadAttachment：saveFile 成功 / 取消 / 大文件错误分类", async () => {
  const bytes = new Uint8Array([1, 2, 3]);
  const readAttachment = async () => ({ bytes, mediaType: "text/plain" });
  const saved: Array<{ data: ArrayBuffer; suggestedName: string }> = [];
  const outcome = await downloadAttachment(
    {
      readAttachment,
      saveFile: async (payload) => {
        saved.push(payload);
        return { canceled: false, success: true, path: "D:/tmp/notes.txt" };
      },
    },
    { sessionId: "s1", ref: "preview/notes.txt", fileName: "notes.txt" },
  );
  assert.equal(outcome.status, "saved");
  assert.equal(saved.length, 1);
  assert.equal(saved[0].suggestedName, "notes.txt");

  const canceled = await downloadAttachment(
    { readAttachment, saveFile: async () => ({ canceled: true, success: false }) },
    { sessionId: "s1", ref: "r", fileName: "n" },
  );
  assert.equal(canceled.status, "canceled");

  const tooLarge = await downloadAttachment(
    {
      readAttachment: async () => {
        throw new Error("fault.attachment.previewTooLarge");
      },
      saveFile: async () => ({ canceled: true, success: false }),
    },
    { sessionId: "s1", ref: "r", fileName: "n" },
  );
  assert.equal(tooLarge.status, "failed");
  assert.equal(tooLarge.status === "failed" ? tooLarge.failure : "", "tooLarge");
});

test("downloadAttachment：无 saveFile 走浏览器下载回调", async () => {
  const triggered: string[] = [];
  const outcome = await downloadAttachment(
    {
      readAttachment: async () => ({ bytes: new Uint8Array([9]), mediaType: "application/pdf" }),
      triggerBrowserDownload: (_bytes, _mediaType, name) => {
        triggered.push(name);
      },
    },
    { sessionId: "s1", ref: "r", fileName: "a.pdf" },
  );
  assert.equal(outcome.status, "browserDownloaded");
  assert.deepEqual(triggered, ["a.pdf"]);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts`
Expected: FAIL —— 模块 `../src/lib/attachmentDownload.js` 不存在。

- [ ] **Step 3: 实现下载模块**

创建 `packages/ui/src/lib/attachmentDownload.ts`：

```ts
// 会话历史附件下载编排：读字节（经权威行授权的 attachmentRead）→ 平台另存/浏览器回退。
// 不请求 mediaType——transport 的 local_path 优化只对 mediaType 生效，下载必须拿完整字节。
import type { AttachmentRef, ConversationRowTarget } from "@mode/shared/mode-protocol-v4";

export type AttachmentDownloadFailure =
  | "tooLarge"
  | "unauthorized"
  | "legacyPeer"
  | "unsupported"
  | "unknown";

export type AttachmentReadResult =
  | { bytes: Uint8Array; mediaType: string }
  | { url: string; mediaType: string };

export interface AttachmentDownloadReadParams {
  sessionId: string;
  ref: string;
  signal?: AbortSignal;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
}

export interface SaveFilePayload {
  data: ArrayBuffer;
  suggestedName: string;
}
export type SaveFileResult = {
  canceled: boolean;
  success: boolean;
  path?: string;
  error?: string;
};
export type SaveFileFn = (payload: SaveFilePayload) => Promise<SaveFileResult>;

export interface DownloadAttachmentDeps {
  readAttachment: (params: AttachmentDownloadReadParams) => Promise<AttachmentReadResult>;
  saveFile?: SaveFileFn;
  triggerBrowserDownload?: (bytes: Uint8Array, mediaType: string, fileName: string) => void;
}

export type AttachmentDownloadOutcome =
  | { status: "saved"; path: string }
  | { status: "browserDownloaded" }
  | { status: "canceled" }
  | { status: "failed"; failure: AttachmentDownloadFailure; message: string };

export interface DownloadAttachmentParams {
  sessionId: string;
  ref: string;
  fileName: string;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
  signal?: AbortSignal;
}

export function classifyAttachmentDownloadError(error: unknown): AttachmentDownloadFailure {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("previewTooLarge")) return "tooLarge";
  if (message.includes("previewRefNotAuthorized")) return "unauthorized";
  // 旧版本对端网关仍卡媒体：previewNotMedia 是兼容分类，不是本端缺陷。
  if (message.includes("previewNotMedia")) return "legacyPeer";
  if (message.includes("readUnsupported")) return "unsupported";
  return "unknown";
}

export function suggestedAttachmentDownloadName(fileName: string): string {
  const withoutSeparators = fileName.replace(/[/\]/gu, "");
  // 剔除控制字符（码点小于空格或 DEL）并去首尾空白；用码点比较而非正则字符类，
  // 避免源码里携带原始控制字节。
  const cleaned = [...withoutSeparators]
    .filter((ch) => ch >= " " && ch.codePointAt(0) !== 0x7f)
    .join("")
    .trim();
  return cleaned.length > 0 ? cleaned : "attachment";
}

export interface BuildAttachmentDownloadReadParamsInput {
  attachment: Pick<AttachmentRef, "ref" | "fileName" | "mime" | "previewRef">;
  sessionId: string;
  rowId: number;
  entityId?: string;
  attachmentIndices?: readonly number[];
  index: number;
  /** artifact 行没有附件序号，传 false 时只带 target（授权走 target 无 index 分支）。 */
  includeAttachmentIndex?: boolean;
}

export function buildAttachmentDownloadReadParams(
  input: BuildAttachmentDownloadReadParamsInput,
): AttachmentDownloadReadParams {
  const includeIndex = input.includeAttachmentIndex ?? true;
  return {
    sessionId: input.sessionId,
    ref: input.attachment.previewRef ?? input.attachment.ref,
    ...(input.entityId ? { target: { rowId: input.rowId, entityId: input.entityId } } : {}),
    ...(input.entityId && includeIndex
      ? { attachmentIndex: input.attachmentIndices?.[input.index] ?? input.index }
      : {}),
  };
}

export async function downloadAttachment(
  deps: DownloadAttachmentDeps,
  params: DownloadAttachmentParams,
): Promise<AttachmentDownloadOutcome> {
  try {
    const result = await deps.readAttachment({
      sessionId: params.sessionId,
      ref: params.ref,
      ...(params.target ? { target: params.target } : {}),
      ...(params.attachmentIndex !== undefined
        ? { attachmentIndex: params.attachmentIndex }
        : {}),
      ...(params.signal ? { signal: params.signal } : {}),
    });
    if ("url" in result) {
      // 不带 mediaType 的读取不应命中 local_path 分支；防御性失败，不伪造下载成功。
      return {
        status: "failed",
        failure: "unsupported",
        message: "attachment read returned local url",
      };
    }
    const suggestedName = suggestedAttachmentDownloadName(params.fileName);
    if (deps.saveFile) {
      const saved = await deps.saveFile({
        data: bytesToArrayBuffer(result.bytes),
        suggestedName,
      });
      if (saved.canceled) return { status: "canceled" };
      if (!saved.success || !saved.path) {
        return { status: "failed", failure: "unknown", message: saved.error ?? "save_failed" };
      }
      return { status: "saved", path: saved.path };
    }
    if (!deps.triggerBrowserDownload) {
      return { status: "failed", failure: "unsupported", message: "no download sink" };
    }
    deps.triggerBrowserDownload(result.bytes, result.mediaType, suggestedName);
    return { status: "browserDownloaded" };
  } catch (error) {
    return {
      status: "failed",
      failure: classifyAttachmentDownloadError(error),
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

function bytesToArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts`
Expected: PASS（6 个 test）

- [ ] **Step 5: 提交**

```bash
git add packages/ui/src/lib/attachmentDownload.ts packages/ui/test/attachmentDownload.test.ts
git commit -m "feat(attachments): UI 附件下载编排与错误分类"
```


---

### Task 3: 历史附件下载入口（文件 pill + 媒体预览对话框）

**Files:**
- Create: `packages/ui/src/hooks/useAttachmentDownload.ts`
- Modify: `packages/ui/src/v4/ConversationRowView.tsx`（`UserInputAttachmentList`）
- Modify: `packages/ui/src/ChatMediaAttachmentPreviewDialog.tsx`
- Modify: `packages/shared/src/test-ids.ts`（`TID_V4_ATTACHMENT_DOWNLOAD`）
- Modify: `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts`

**Interfaces:**
- Consumes: Task 2 全部导出；`usePlatform`（`@/hooks/usePlatform.js`）、`toast`（`@/components/ui/toast.js`）、`useModeIntl`、`ConversationRowRenderContext["readAttachment"]`（`packages/ui/src/v4/conversationRowContext.ts:181`）
- Produces: `useAttachmentDownload(options: { sessionId?: string; readAttachment?: NonNullable<ConversationRowRenderContext["readAttachment"]> }): { download(input): void; downloadingRefs: ReadonlySet<string> }`，`download` 入参 `{ attachment: Pick<AttachmentRef, "ref"|"fileName"|"mime"|"previewRef">; rowId: number; entityId?: string; attachmentIndices?: readonly number[]; index: number; includeAttachmentIndex?: boolean }`。Task 4 复用同一 hook。

- [ ] **Step 1: i18n 与 test id（后续步骤的依赖）**

`packages/shared/src/test-ids.ts` 在 `TID_V4_ROW_ATTACHMENTS`（711 行）附近追加：

```ts
export const TID_V4_ATTACHMENT_DOWNLOAD = "v4-attachment-download";
```

三个 locale 文件追加以下 key（插到既有 `chat.attachments.` 键簇旁，格式与相邻行一致）：

| key | zh-CN | en-US | fa |
| --- | --- | --- | --- |
| `chat.attachments.download` | `下载` | `Download` | `دانلود` |
| `chat.attachments.downloadFailed` | `下载失败` | `Download failed` | `دانلود ناموفق` |
| `chat.attachments.downloadStarted` | `已开始下载` | `Download started` | `دانلود آغاز شد` |
| `chat.attachments.downloadSucceeded` | `已保存到 {path}` | `Saved to {path}` | `ذخیره شد در {path}` |
| `chat.attachments.downloadTooLarge` | `附件超过会话内下载上限（20 MiB，视频 30 MiB），请到工作区直接取用` | `Attachment exceeds the in-conversation download limit (20 MiB; 30 MiB for video). Retrieve it from the workspace instead.` | `پیوست از سقف دانلود در مکالمه بیشتر است (20 می‌بایت؛ ویدیو 30 می‌بایت). از فضای کاری دریافت کنید.` |
| `chat.attachments.downloadUnauthorized` | `附件已不可用` | `Attachment no longer available` | `پیوست در دسترس نیست` |
| `chat.attachments.downloadLegacyPeer` | `对端版本过低，无法下载该附件` | `Peer version is too old to download this attachment` | `نسخه سمت مقابل برای دانلود این پیوست قدیمی است` |

- [ ] **Step 2: 实现下载 hook**

创建 `packages/ui/src/hooks/useAttachmentDownload.ts`：

```ts
import { useCallback, useState } from "react";
import { toast } from "@/components/ui/toast.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import type { AttachmentRef } from "@mode/shared/mode-protocol-v4";
import {
  buildAttachmentDownloadReadParams,
  downloadAttachment,
} from "@/lib/attachmentDownload.js";
import type { ConversationRowRenderContext } from "@/v4/conversationRowContext.js";

type ReadAttachmentFn = NonNullable<ConversationRowRenderContext["readAttachment"]>;

export interface AttachmentDownloadInput {
  attachment: Pick<AttachmentRef, "ref" | "fileName" | "mime" | "previewRef">;
  rowId: number;
  entityId?: string;
  attachmentIndices?: readonly number[];
  index: number;
  /** artifact 行等无附件序号场景传 false。 */
  includeAttachmentIndex?: boolean;
}

/** 历史附件下载：读字节 → 平台另存（Electron 原生对话框）/ 浏览器回退；toast 收口结果。 */
export function useAttachmentDownload(options: {
  sessionId?: string;
  readAttachment?: ReadAttachmentFn;
}) {
  const { sessionId, readAttachment } = options;
  const platform = usePlatform();
  const { intl } = useModeIntl();
  const [downloadingRefs, setDownloadingRefs] = useState<ReadonlySet<string>>(() => new Set());

  const triggerBrowserDownload = useCallback(
    (bytes: Uint8Array, mediaType: string, fileName: string) => {
      const blob = new Blob([bytes.slice()], { type: mediaType });
      const objectUrl = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = objectUrl;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(objectUrl);
    },
    [],
  );

  const download = useCallback(
    (input: AttachmentDownloadInput) => {
      if (!sessionId || !readAttachment) return;
      const ref = input.attachment.previewRef ?? input.attachment.ref;
      setDownloadingRefs((current) => new Set(current).add(ref));
      const readParams = buildAttachmentDownloadReadParams({
        attachment: input.attachment,
        sessionId,
        rowId: input.rowId,
        ...(input.entityId ? { entityId: input.entityId } : {}),
        ...(input.attachmentIndices ? { attachmentIndices: input.attachmentIndices } : {}),
        index: input.index,
        ...(input.includeAttachmentIndex !== undefined
          ? { includeAttachmentIndex: input.includeAttachmentIndex }
          : {}),
      });
      void downloadAttachment(
        {
          readAttachment: (params) => readAttachment(params),
          ...(platform?.saveFile ? { saveFile: (payload) => platform.saveFile!(payload) } : {}),
          triggerBrowserDownload,
        },
        {
          sessionId: readParams.sessionId,
          ref: readParams.ref,
          fileName: input.attachment.fileName,
          ...(readParams.target ? { target: readParams.target } : {}),
          ...(readParams.attachmentIndex !== undefined
            ? { attachmentIndex: readParams.attachmentIndex }
            : {}),
        },
      )
        .then((outcome) => {
          if (outcome.status === "saved") {
            toast(
              intl.formatMessage(
                { id: "chat.attachments.downloadSucceeded" },
                { path: outcome.path },
              ),
            );
            return;
          }
          if (outcome.status === "browserDownloaded") {
            toast(intl.formatMessage({ id: "chat.attachments.downloadStarted" }));
            return;
          }
          if (outcome.status === "canceled") return;
          const failureKey =
            outcome.failure === "tooLarge"
              ? "chat.attachments.downloadTooLarge"
              : outcome.failure === "unauthorized"
                ? "chat.attachments.downloadUnauthorized"
                : outcome.failure === "legacyPeer"
                  ? "chat.attachments.downloadLegacyPeer"
                  : "chat.attachments.downloadFailed";
          toast(intl.formatMessage({ id: failureKey }), { variant: "destructive" });
          logger.warn("[attachment-download] 附件下载失败", {
            ref,
            failure: outcome.failure,
            message: outcome.message,
          });
        })
        .finally(() => {
          setDownloadingRefs((current) => {
            const next = new Set(current);
            next.delete(ref);
            return next;
          });
        });
    },
    [intl, platform, readAttachment, sessionId, triggerBrowserDownload],
  );

  return { download, downloadingRefs };
}
```

说明：`readAttachment` 的实际参数类型是 `ConversationAttachmentReadParams`（比本模块 `AttachmentDownloadReadParams` 多可选 `mediaType`）——若 typecheck 报不兼容，按 `ConversationRowRenderContext` 的真实签名对齐类型（以 `pnpm typecheck` 输出为准修正，不改变行为）。

- [ ] **Step 3: 接线 UserInputAttachmentList 文件 pill 下载**

`ConversationRowView.tsx` 修改：

1. import 追加：`DownloadIcon`（加进既有 lucide icon 块）、`TID_V4_ATTACHMENT_DOWNLOAD`（`@mode/shared` 块）、`useAttachmentDownload`（`@/hooks/useAttachmentDownload.js`）。
2. `UserInputAttachmentList` 组件顶部 hooks 区追加：

```ts
  const { download, downloadingRefs } = useAttachmentDownload({ sessionId, readAttachment });
  const downloadLabel = intl.formatMessage({ id: "chat.attachments.download" });
```

3. 在 `items` 的 `map` 中 `<Attachment ...>` JSX 内、`FileDisplayInline` 分支（sent 文件 pill）之后、`{onRemove && isEditingAttachment ? ...}` 之前插入：

```tsx
        {!isEditingAttachment && sessionId && readAttachment ? (
          <button
            type="button"
            data-testid={testId(TID_V4_ATTACHMENT_DOWNLOAD, `${rowId}-${index}`)}
            aria-label={downloadLabel}
            title={downloadLabel}
            disabled={downloadingRefs.has(ref)}
            className="ms-1 inline-flex size-5 shrink-0 items-center justify-center rounded text-foreground-subtle transition hover:text-foreground disabled:opacity-50"
            onClick={(event) => {
              // pill 自身可能可打开预览（pdf）；下载点击不得连带触发。
              event.stopPropagation();
              download({
                attachment,
                rowId,
                ...(entityId ? { entityId } : {}),
                ...(attachmentIndices ? { attachmentIndices } : {}),
                index,
              });
            }}
          >
            <DownloadIcon className="size-3.5" />
          </button>
        ) : null}
```

4. 媒体预览对话框挂接：`UserInputAttachmentList` 新增状态：

```ts
  const [previewDownload, setPreviewDownload] = useState<{
    attachment: (typeof attachments)[number];
    index: number;
  } | null>(null);
```

在 `openVideoPreview(attachment, persistedIndex, ...)` 与 `openPdfPreview(attachment, persistedIndex)` 函数体首行各加 `setPreviewDownload({ attachment, index: persistedIndex })`（以两函数实际 index 参数名为准）；对话框关闭（`closeVideoPreview()`、pdf 对话框 `onOpenChange(false)`）时 `setPreviewDownload(null)`。

video 与 pdf 两个 `<ChatMediaAttachmentPreviewDialog ...>` 调用点各追加两个 props（其余既有 props 不动）：

```tsx
        onDownload={
          previewDownload && sessionId && readAttachment
            ? () =>
                download({
                  attachment: previewDownload.attachment,
                  rowId,
                  ...(entityId ? { entityId } : {}),
                  ...(attachmentIndices ? { attachmentIndices } : {}),
                  index: previewDownload.index,
                })
            : undefined
        }
        downloading={
          previewDownload
            ? downloadingRefs.has(
                previewDownload.attachment.previewRef ?? previewDownload.attachment.ref,
              )
            : false
        }
```

- [ ] **Step 4: ChatMediaAttachmentPreviewDialog 下载按钮**

`ChatMediaAttachmentPreviewDialog.tsx`：

1. import 追加 `DownloadIcon`（lucide-react）、`Button`（`@/components/ui/button.js`）、`testId`/`TID_V4_ATTACHMENT_DOWNLOAD`（`@mode/shared`）。
2. props 增加：

```ts
  onDownload?: () => void;
  downloading?: boolean;
```

3. 组件内取 `const downloadLabel = intl.formatMessage({ id: "chat.attachments.download" });`，并在 `DialogHeader` 内（`DialogDescription` 之后）渲染：

```tsx
          {onDownload ? (
            <Button
              type="button"
              size="icon-md"
              variant="ghost"
              aria-label={downloadLabel}
              disabled={downloading}
              onClick={onDownload}
              className="absolute end-12 top-3 z-10"
              data-testid={testId(TID_V4_ATTACHMENT_DOWNLOAD, "preview")}
            >
              <DownloadIcon className="size-4" />
            </Button>
          ) : null}
```

若 `DialogContent` 无 relative 定位（按钮不跟随弹窗），改放 `DialogHeader` 行内 `ms-auto shrink-0`（以实际渲染为准，确保不与关闭按钮重叠）。

- [ ] **Step 5: 验证并提交**

Run: `pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts && pnpm typecheck && pnpm lint`
Expected: PASS（typecheck/lint 如实报告）

```bash
git add packages/ui packages/shared/src/test-ids.ts
git commit -m "feat(ui): 会话历史附件下载入口（文件 pill 与媒体预览）"
```

---

### Task 4: artifact 行下载（agent 产出文件）

**Files:**
- Modify: `packages/ui/src/v4/ConversationRowView.tsx`（`ArtifactRowView` 与行分发 `case "artifact"`）

**Interfaces:**
- Consumes: Task 3 的 `useAttachmentDownload`（`includeAttachmentIndex: false`）
- Produces: `ArtifactRowView` 组件签名变为 `{ row, context }`（唯一调用方 = 行分发处）。

- [ ] **Step 1: ArtifactRowView 增加下载动作**

`ConversationRowView.tsx` 的 `ArtifactRowView`（约 151 行）整体替换为：

```ts
const ArtifactRowView = memo(function ArtifactRowView({
  row,
  context,
}: {
  row: ArtifactRow;
  context: ConversationRowRenderContext;
}) {
  const { intl } = useModeIntl();
  const sessionId = context.sessionId ?? undefined;
  const { download, downloadingRefs } = useAttachmentDownload({
    sessionId,
    readAttachment: context.readAttachment,
  });
  const downloadLabel = intl.formatMessage({ id: "chat.attachments.download" });
  return (
    <RowShell rowId={row.rowId} className="px-4 py-1">
      <div className="flex items-center gap-2 rounded-lg border border-card-border bg-card px-3 py-2">
        <FileIcon className="size-4 shrink-0 text-foreground-subtle" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <p className="truncate text-ui-base text-foreground">{row.displayName}</p>
          <p className="text-ui-sm text-foreground-subtle">
            {row.artifactType.toUpperCase()} · {row.sizeBytes} bytes
          </p>
        </div>
        {sessionId && context.readAttachment ? (
          <button
            type="button"
            data-testid={testId(TID_V4_ATTACHMENT_DOWNLOAD, `artifact-${row.rowId}`)}
            aria-label={downloadLabel}
            title={downloadLabel}
            disabled={downloadingRefs.has(row.ref)}
            className="inline-flex size-6 shrink-0 items-center justify-center rounded text-foreground-subtle transition hover:text-foreground disabled:opacity-50"
            onClick={() =>
              download({
                attachment: {
                  ref: row.ref,
                  fileName: row.displayName,
                  mime: row.mimeType,
                },
                rowId: row.rowId,
                ...(row.entityId ? { entityId: row.entityId } : {}),
                index: 0,
                // artifact 行没有附件序号；授权走 target 无 index / 扫描分支。
                includeAttachmentIndex: false,
              })
            }
          >
            <DownloadIcon className="size-4" />
          </button>
        ) : null}
      </div>
    </RowShell>
  );
});
```

（`ConversationRowRenderContext` 若文件内未 import，从 `@/v4/conversationRowContext.js` 引入；`useModeIntl`/`testId`/`DownloadIcon` 以文件既有 import 为准补齐。）

行分发处改为：

```ts
    case "artifact":
      return <ArtifactRowView row={row} context={context} />;
```

- [ ] **Step 2: 验证并提交**

Run: `pnpm typecheck && pnpm lint && pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts`
Expected: PASS

```bash
git add packages/ui/src/v4/ConversationRowView.tsx
git commit -m "feat(ui): artifact 行附件下载"
```

---

### Task 5: SSH materialize 排序判定（不变量 1 兜底）

**Files:**
- Modify: `packages/desktop/src/host/remotePromptAttachments.ts`
- Create: `packages/desktop/tests/fixtures/remote-prompt-attachments-order.ts`
- Create: `packages/desktop/tests/remote-prompt-attachments-order.test.mts`

**Interfaces:**
- Consumes: 既有 `materializeRemotePromptAttachments(input, { backend })`；`IRemoteBackend`、`quotePosixPathArg`
- Produces: `materializeRemotePromptAttachments` 四级排序（staging 根放行 → 本机存在上传 → 仅目标机存在放行 → 双缺报错）；新增可选依赖 `options.statFile?`、`options.remoteFileExists?`（缺省接 node fs / backend exec）；导出 `isPathInRoot`、`replaceContentPaths`（Task 7 Consumes）。

- [ ] **Step 1: 写失败夹具**

创建 `packages/desktop/tests/fixtures/remote-prompt-attachments-order.ts`：

```ts
/**
 * SSH materialize 排序判定夹具（spec: remote-workspace-bidirectional-attachments 不变量 1）。
 * 通过后打印 REMOTE_MATERIALIZE_ORDER_OK。
 */
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { materializeRemotePromptAttachments } from "../../src/host/remotePromptAttachments.js";

type BackendStream = {
  stdout: { on(event: "data", cb: (chunk: Buffer | string) => void): void };
  stderr: { on(event: "data", cb: () => void): void };
  on(event: "close", cb: (result: { code: number }) => void): void;
};

function streamResult(code: number, stdout = ""): BackendStream {
  return {
    stdout: {
      on(_event, cb) {
        if (stdout) cb(stdout);
      },
    },
    stderr: { on: () => undefined },
    on(event, cb) {
      if (event === "close") cb({ code });
    },
  } as unknown as BackendStream;
}

async function main() {
  const dir = await mkdtemp(join(tmpdir(), "mat-order-"));
  try {
    const localFile = join(dir, "local.txt");
    await writeFile(localFile, "host bytes");

    const uploads: Array<{ from: string; to: string }> = [];
    // test -f 探测：只对「远端存在清单」里的路径返回 0。
    const remoteOnlyPaths = new Set(["/remote-only/data.bin"]);
    const backend = {
      exec(command: string) {
        if (command.startsWith("printf")) return streamResult(0, "/home/u\n");
        if (command.includes("test -f")) {
          const path = command.slice(command.indexOf("test -f") + "test -f ".length).trim();
          return streamResult(remoteOnlyPaths.has(path) ? 0 : 1);
        }
        return streamResult(0);
      },
      async upload(from: string, to: string) {
        uploads.push({ from, to });
      },
    } as never;

    // 1) 本机存在 → 上传改写（既有行为保持）
    const uploaded = await materializeRemotePromptAttachments(
      {
        taskId: "t1",
        traceId: "trace-1",
        content: `see ${localFile}`,
        attachments: [
          {
            kind: "file",
            filename: "local.txt",
            mimeType: "text/plain",
            sizeBytes: 10,
            localPath: localFile,
          },
        ] as never,
      },
      { backend },
    );
    assert.equal(uploads.length, 1);
    assert.notEqual(uploaded.attachments?.[0]?.localPath, localFile);
    assert.ok(!uploaded.content.includes(localFile), "内容中的本机路径必须被改写");

    // 2) 本机不存在、目标机存在 → 原样放行（不上传、不改写）
    const passThrough = await materializeRemotePromptAttachments(
      {
        taskId: "t1",
        traceId: "trace-2",
        content: "",
        attachments: [
          {
            kind: "file",
            filename: "d.bin",
            mimeType: "application/octet-stream",
            sizeBytes: 1,
            localPath: "/remote-only/data.bin",
          },
        ] as never,
      },
      { backend },
    );
    assert.equal(uploads.length, 1, "已存在目标机的路径不得再次上传");
    assert.equal(passThrough.attachments?.[0]?.localPath, "/remote-only/data.bin");

    // 3) 两侧都不存在 → 可读错误
    await assert.rejects(
      materializeRemotePromptAttachments(
        {
          taskId: "t1",
          traceId: "trace-3",
          content: "",
          attachments: [
            {
              kind: "file",
              filename: "x",
              mimeType: "text/plain",
              sizeBytes: 1,
              localPath: "/nowhere/x.txt",
            },
          ] as never,
        },
        { backend },
      ),
      /附件路径在本机和远端都不存在/u,
    );

    // 4) staging 根路径 → 透传（既有行为保持）
    const staged = await materializeRemotePromptAttachments(
      {
        taskId: "t1",
        traceId: "trace-4",
        content: "",
        attachments: [
          {
            kind: "file",
            filename: "s.txt",
            mimeType: "text/plain",
            sizeBytes: 1,
            localPath: "~/.mode/tmp/prompt-attachments/t/n/1-s.txt",
          },
        ] as never,
      },
      { backend },
    );
    assert.equal(
      staged.attachments?.[0]?.localPath,
      "~/.mode/tmp/prompt-attachments/t/n/1-s.txt",
    );

    // 5) 无 localPath（dataBase64 内联）→ 放行不变
    const inline = await materializeRemotePromptAttachments(
      {
        taskId: "t1",
        traceId: "trace-5",
        content: "",
        attachments: [
          { kind: "image", filename: "p.png", mimeType: "image/png", dataBase64: "AAAA" },
        ] as never,
      },
      { backend },
    );
    assert.equal(inline.attachments?.[0]?.dataBase64, "AAAA");

    console.log("REMOTE_MATERIALIZE_ORDER_OK");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

await main();
```

创建 `packages/desktop/tests/remote-prompt-attachments-order.test.mts`：

```ts
/**
 * 远端 materialize 排序判定回归。运行：
 * node --test packages/desktop/tests/remote-prompt-attachments-order.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

test("materialize 排序判定：本机存在上传 / 仅目标机放行 / 两侧缺失报错", async () => {
  const fixture = fileURLToPath(
    new URL("./fixtures/remote-prompt-attachments-order.ts", import.meta.url),
  );
  const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
  const child = spawn(process.execPath, ["--import", "tsx", fixture], {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => (output += String(chunk)));
  child.stderr.on("data", (chunk) => (output += String(chunk)));
  const exitCode = await new Promise<number>((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`夹具进程 30s 未退出\n${output}`));
    }, 30_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve(code ?? 1);
    });
  });
  assert.equal(exitCode, 0, `夹具失败：\n${output}`);
  assert.match(output, /REMOTE_MATERIALIZE_ORDER_OK/u, `夹具未完成断言：\n${output}`);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `node --test packages/desktop/tests/remote-prompt-attachments-order.test.mts`
Expected: FAIL —— 场景 2 当前会走 `backend.upload`（本地无此路径但 mock upload 不校验），或场景 3 不抛「附件路径在本机和远端都不存在」。

- [ ] **Step 3: 实现排序判定**

`packages/desktop/src/host/remotePromptAttachments.ts`：

1. import 增加 `import { stat } from "node:fs/promises";`
2. 纯助手加 `export`：`isPathInRoot`、`replaceContentPaths`（实现不动）。
3. options 类型扩展：

```ts
  options: {
    backend: Pick<IRemoteBackend, "exec" | "upload">;
    uploadOptions?: RemoteUploadOptions;
    /** 测试注入点；缺省走 node fs stat（本机存在性判定）。 */
    statFile?: (path: string) => Promise<boolean>;
    /** 测试注入点；缺省走 backend exec `test -f`（目标机存在性判定）。 */
    remoteFileExists?: (path: string) => Promise<boolean>;
  };
```

4. 新增默认实现（放 `getAttachmentLocalPath` 附近）：

```ts
async function hostFileExists(path: string): Promise<boolean> {
  try {
    const info = await stat(path);
    return info.isFile();
  } catch {
    return false;
  }
}

async function targetFileExists(
  backend: Pick<IRemoteBackend, "exec">,
  path: string,
): Promise<boolean> {
  try {
    await waitForRemoteCommand(backend, `test -f ${quotePosixPathArg(path)}`);
    return true;
  } catch {
    return false;
  }
}
```

5. 主循环在既有 staging 根放行（`isPathInRoot` 两处判断）之后、`buildRemotePromptAttachmentPath` 上传之前插入：

```ts
    const existsOnHost = options.statFile
      ? await options.statFile(localPath)
      : await hostFileExists(localPath);
    if (!existsOnHost) {
      const existsOnTarget = options.remoteFileExists
        ? await options.remoteFileExists(localPath)
        : await targetFileExists(options.backend, localPath);
      if (existsOnTarget) {
        // 该路径本就属于目标机（遗留转换把远端路径落成 localPath）：
        // 不读、不传，原样放行——目标机 agent 直接可读，不变量 1 不破。
        nextAttachments.push(attachment);
        continue;
      }
      throw new Error(`附件路径在本机和远端都不存在：${localPath}`);
    }
```

（中文注释说明排序依据：本机优先、目标机兜底、双缺 fail-closed。）

- [ ] **Step 4: 运行夹具确认通过**

Run: `node --test packages/desktop/tests/remote-prompt-attachments-order.test.mts`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add packages/desktop/src/host/remotePromptAttachments.ts packages/desktop/tests/remote-prompt-attachments-order.test.mts packages/desktop/tests/fixtures/remote-prompt-attachments-order.ts
git commit -m "fix(desktop): 远端 materialize 按本机/目标机存在性排序判定"
```

---

### Task 6: LAN stage —— 附件 chunk 通道上传

**Files:**
- Create: `packages/desktop/src/host/lanPromptAttachments.ts`（stage 部分）
- Create: `packages/desktop/tests/fixtures/lan-prompt-attachments.ts`
- Create: `packages/desktop/tests/lan-prompt-attachments.test.mts`

**Interfaces:**
- Consumes: `IPromptAttachmentTransferService` / `PromptAttachmentStageParams` / `PromptAttachmentStageResult`（`@mode/services`）、`IModeAgentService.attachmentBeginV4/ChunkV4/CommitV4/AbortV4`（`ModeAgentAttachment*Params` = `ModeAgentSessionTarget{workspacePath, workspaceIdentity?, sessionId}` + 传输字段；connectionId 由 connection scope 以 trusted carrier 注入）、`PROTOCOL_V4_LIMITS`、`Emitter`（`@mode/rpc`，per-operation emitter 模式抄 `packages/desktop/src/host/promptAttachmentTransferService.ts`）
- Produces: `createLanPromptAttachmentTransferService(deps: { channel: LanAttachmentChannel }): IPromptAttachmentTransferService`；`LanAttachmentChannel = Pick<IModeAgentService, "attachmentBeginV4" | "attachmentChunkV4" | "attachmentCommitV4" | "attachmentAbortV4">`。Task 7 复用 `LanAttachmentChannel`；Task 8 消费工厂。

- [ ] **Step 1: 写失败夹具**

创建 `packages/desktop/tests/fixtures/lan-prompt-attachments.ts`：

```ts
/**
 * LAN stage 夹具：分块重组、校验和、幂等 begin、超限拒绝、失败 abort、
 * fault 透传、进度事件。通过后打印 LAN_STAGE_OK。
 */
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, rm, writeFile, truncate } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PROTOCOL_V4_LIMITS } from "@mode/shared/mode-protocol-v4";
import { createLanPromptAttachmentTransferService } from "../../src/host/lanPromptAttachments.js";

async function main() {
  const dir = await mkdtemp(join(tmpdir(), "lan-stage-"));
  try {
    const filePath = join(dir, "payload.bin");
    const payload = randomBytes(Math.floor(1.2 * 1024 * 1024));
    await writeFile(filePath, payload);
    const expectedChecksum = `sha256:${createHash("sha256").update(payload).digest("hex")}`;

    const received: Uint8Array[] = [];
    let beginCalls = 0;
    let abortCalls = 0;
    let beginState: "staging" | "committed" = "staging";
    const committedRef = "mode-artifact://s1/upload-1";

    const channel = {
      async attachmentBeginV4(params: {
        totalBytes: number;
        totalChunks: number;
        checksum: string;
      }) {
        beginCalls += 1;
        assert.equal(params.checksum, expectedChecksum);
        assert.equal(params.totalBytes, payload.byteLength);
        assert.equal(
          params.totalChunks,
          Math.ceil(payload.byteLength / PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes),
        );
        if (beginState === "committed") {
          return { state: "committed" as const, nextChunkIndex: 0, ref: committedRef };
        }
        return { state: "staging" as const, nextChunkIndex: 0 };
      },
      async attachmentChunkV4(params: { chunkIndex: number; dataBase64: string }) {
        received[params.chunkIndex] = Buffer.from(params.dataBase64, "base64");
        return { nextChunkIndex: params.chunkIndex + 1 };
      },
      async attachmentCommitV4() {
        const total = received.reduce((sum, part) => sum + part.byteLength, 0);
        assert.equal(total, payload.byteLength, "重组字节数必须等于原文件");
        const rebuilt = Buffer.concat(received.map((part) => Buffer.from(part)));
        assert.ok(rebuilt.equals(payload), "重组字节必须逐位一致");
        return { ref: committedRef };
      },
      async attachmentAbortV4() {
        abortCalls += 1;
      },
    } as never;

    const service = createLanPromptAttachmentTransferService({ channel });
    const progress: string[] = [];
    const dispose = service.onDynamicProgress("op-1")((event) => {
      progress.push(event.phase);
    });

    // A) 正常上传
    const result = await service.stage({
      operationId: "op-1",
      sessionId: "s1",
      workspacePath: "/ws",
      localPath: filePath,
      fileName: "payload.bin",
      mime: "application/octet-stream",
      sizeBytes: payload.byteLength,
    });
    assert.equal(result.staged, true);
    assert.equal(result.ref, committedRef);
    assert.equal(result.bytes, payload.byteLength);
    assert.ok(progress.includes("uploading"), "必须发出 uploading 进度");
    assert.ok(progress.includes("committing"), "必须发出 committing 进度");
    assert.ok(progress.includes("complete"), "必须发出 complete 进度");
    dispose.dispose();

    // B) 幂等 begin：对端已 committed → 直接返回 ref
    beginState = "committed";
    beginCalls = 0;
    const idempotent = await service.stage({
      operationId: "op-1",
      sessionId: "s1",
      workspacePath: "/ws",
      localPath: filePath,
      fileName: "payload.bin",
      mime: "application/octet-stream",
      sizeBytes: payload.byteLength,
    });
    assert.equal(idempotent.ref, committedRef);
    assert.equal(beginCalls, 1);
    beginState = "staging";

    // C) 超 20 MiB：不发起 begin，直接可读错误
    const oversized = join(dir, "big.bin");
    await writeFile(oversized, "");
    await truncate(oversized, PROTOCOL_V4_LIMITS.attachmentMaxBytes + 1);
    beginCalls = 0;
    await assert.rejects(
      service.stage({
        operationId: "op-2",
        sessionId: "s1",
        workspacePath: "/ws",
        localPath: oversized,
        fileName: "big.bin",
        mime: "application/octet-stream",
      }),
      /局域网连接附件单文件上限/u,
    );
    assert.equal(beginCalls, 0, "超限文件不得发起事务");

    // D) chunk 失败 → abort 尝试 + 「局域网附件上传失败」包装
    const failChannel = {
      async attachmentBeginV4() {
        return { state: "staging" as const, nextChunkIndex: 0 };
      },
      async attachmentChunkV4() {
        throw new Error("ECONNRESET");
      },
      async attachmentCommitV4() {
        throw new Error("unreachable");
      },
      async attachmentAbortV4() {
        abortCalls += 1;
      },
    } as never;
    const failService = createLanPromptAttachmentTransferService({ channel: failChannel });
    const before = abortCalls;
    await assert.rejects(
      failService.stage({
        operationId: "op-3",
        sessionId: "s1",
        workspacePath: "/ws",
        localPath: filePath,
        fileName: "payload.bin",
        mime: "application/octet-stream",
      }),
      /局域网附件上传失败/u,
    );
    assert.equal(abortCalls, before + 1, "失败必须尽力 abort");

    // E) putUnsupported 稳定码 → 中文可读文案（不被通用包装掩盖）
    const faultChannel = {
      async attachmentBeginV4() {
        throw new Error("fault.attachment.putUnsupported");
      },
      async attachmentChunkV4() {
        throw new Error("unreachable");
      },
      async attachmentCommitV4() {
        throw new Error("unreachable");
      },
      async attachmentAbortV4() {},
    } as never;
    const faultService = createLanPromptAttachmentTransferService({ channel: faultChannel });
    await assert.rejects(
      faultService.stage({
        operationId: "op-4",
        sessionId: "s1",
        workspacePath: "/ws",
        localPath: filePath,
        fileName: "payload.bin",
        mime: "application/octet-stream",
      }),
      (error: Error) => error.message.includes("对端版本不支持附件上传"),
    );

    console.log("LAN_STAGE_OK");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

await main();
```

`packages/desktop/tests/lan-prompt-attachments.test.mts`：结构同 Task 5 的 wrapper（spawn `node --import tsx` 夹具 + 30s 超时 + exitCode 断言），test 名改为「LAN stage：分块重组 / 幂等 begin / 超限拒绝 / 失败 abort / fault 透传」，fixture 指向 `./fixtures/lan-prompt-attachments.ts`，marker 断言 `/LAN_STAGE_OK/`。

- [ ] **Step 2: 运行确认失败**

Run: `node --test packages/desktop/tests/lan-prompt-attachments.test.mts`
Expected: FAIL —— `../../src/host/lanPromptAttachments.js` 不存在。

- [ ] **Step 3: 实现 LAN stage**

创建 `packages/desktop/src/host/lanPromptAttachments.ts`：

```ts
// LAN 远端工作区附件传输：对端是已运行的 Mode 实例、无 stdio backend，
// 没有 backend.upload 可用；改走会话 artifact 的 begin/chunk/commit 字节通道
// （与手机远控同一条已验证链路），对 renderer 仍暴露同一个 stage 契约。
// 单文件上限 = PROTOCOL_V4_LIMITS.attachmentMaxBytes（20 MiB，spec 已确认不调整）。
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { Emitter } from "@mode/rpc";
import { PROTOCOL_V4_LIMITS } from "@mode/shared/mode-protocol-v4";
import type {
  IPromptAttachmentTransferService,
  IModeAgentService,
  PromptAttachmentStageParams,
  PromptAttachmentStageResult,
  PromptAttachmentTransferProgress,
} from "@mode/services";

export type LanAttachmentChannel = Pick<
  IModeAgentService,
  "attachmentBeginV4" | "attachmentChunkV4" | "attachmentCommitV4" | "attachmentAbortV4"
>;

const CHUNK_BYTES = PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes;
const LAN_STAGE_LIMIT_MESSAGE =
  "局域网连接附件单文件上限 20 MiB，请改用 SSH/WSL/Docker 工作区，或先把文件放到对端工作区。";
const LAN_PUT_UNSUPPORTED_MESSAGE = "对端版本不支持附件上传";

interface LanStageTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  sessionId: string;
}

interface LanStageRecord {
  controller: AbortController;
  target: LanStageTarget;
  committed: boolean;
}

/** uploadId 必须匹配 ^[A-Za-z0-9][A-Za-z0-9._:-]*$（≤128）；operationId 来自 nanoid，先归一。 */
function toUploadId(operationId: string): string {
  const cleaned = operationId.replace(/[^A-Za-z0-9._:-]+/gu, "-");
  const started = /^[A-Za-z0-9]/u.test(cleaned) ? cleaned : `u-${cleaned}`;
  return started.slice(0, 128);
}

async function sha256OfFile(path: string): Promise<string> {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) {
    hash.update(chunk as Buffer);
  }
  return hash.digest("hex");
}

async function readRange(path: string, start: number, end: number): Promise<Buffer> {
  const parts: Buffer[] = [];
  for await (const piece of createReadStream(path, { start, end })) {
    parts.push(piece as Buffer);
  }
  return Buffer.concat(parts);
}

function abortError(): Error {
  return Object.assign(new Error("LAN attachment stage canceled"), { name: "AbortError" });
}

function isAbortLike(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

export function createLanPromptAttachmentTransferService(deps: {
  channel: LanAttachmentChannel;
}): IPromptAttachmentTransferService {
  const records = new Map<string, LanStageRecord>();
  const emitters = new Map<string, Emitter<PromptAttachmentTransferProgress>>();

  const getEmitter = (operationId: string) => {
    const existing = emitters.get(operationId);
    if (existing) return existing;
    const emitter = new Emitter<PromptAttachmentTransferProgress>({
      onDidRemoveLastListener: () => {
        emitters.delete(operationId);
        emitter.dispose();
      },
    });
    emitters.set(operationId, emitter);
    return emitter;
  };

  const abortRemote = async (target: LanStageTarget, uploadId: string) => {
    await deps.channel.attachmentAbortV4({ ...target, uploadId }).catch(() => {});
  };

  const cancelInternal = async (operationId: string) => {
    const record = records.get(operationId);
    if (!record) return;
    record.controller.abort();
    records.delete(operationId);
    await abortRemote(record.target, toUploadId(operationId));
    getEmitter(operationId).fire({
      operationId,
      phase: "canceled",
      uploadedBytes: 0,
      totalBytes: 0,
    });
  };

  return {
    async stage(params: PromptAttachmentStageParams): Promise<PromptAttachmentStageResult> {
      await cancelInternal(params.operationId);
      const target: LanStageTarget = {
        workspacePath: params.workspacePath,
        ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
        sessionId: params.sessionId,
      };
      const record: LanStageRecord = {
        controller: new AbortController(),
        target,
        committed: false,
      };
      records.set(params.operationId, record);
      const emitter = getEmitter(params.operationId);
      const uploadId = toUploadId(params.operationId);
      try {
        const info = await stat(params.localPath).catch((error: unknown) => {
          const message = error instanceof Error ? error.message : String(error);
          if (message.includes("ENOENT")) {
            throw new Error(`附件文件不存在：${params.localPath}`);
          }
          throw error;
        });
        if (!info.isFile()) throw new Error(`附件不是文件：${params.fileName}`);
        if (info.size > PROTOCOL_V4_LIMITS.attachmentMaxBytes) {
          throw new Error(LAN_STAGE_LIMIT_MESSAGE);
        }
        const totalBytes = info.size;
        const checksum = `sha256:${await sha256OfFile(params.localPath)}`;
        const totalChunks = totalBytes === 0 ? 0 : Math.ceil(totalBytes / CHUNK_BYTES);
        const begin = await deps.channel.attachmentBeginV4({
          ...target,
          uploadId,
          fileName: params.fileName,
          mime: params.mime,
          totalBytes,
          totalChunks,
          checksum,
        });

        let ref: string;
        if (begin.state === "committed") {
          // 幂等续传：begin ACK 丢失后重试，对端已提交则直接采用既有 ref。
          ref = begin.ref;
        } else {
          let uploadedBytes = 0;
          emitter.fire({
            operationId: params.operationId,
            phase: "uploading",
            uploadedBytes,
            totalBytes,
          });
          for (let chunkIndex = begin.nextChunkIndex; chunkIndex < totalChunks; chunkIndex += 1) {
            if (record.controller.signal.aborted || records.get(params.operationId) !== record) {
              throw abortError();
            }
            const start = chunkIndex * CHUNK_BYTES;
            const end = Math.min(start + CHUNK_BYTES, totalBytes) - 1;
            const dataBase64 = (await readRange(params.localPath, start, end)).toString("base64");
            if (record.controller.signal.aborted || records.get(params.operationId) !== record) {
              throw abortError();
            }
            await deps.channel.attachmentChunkV4({
              ...target,
              uploadId,
              chunkIndex,
              dataBase64,
            });
            uploadedBytes = end + 1;
            emitter.fire({
              operationId: params.operationId,
              phase: "uploading",
              uploadedBytes,
              totalBytes,
            });
          }
          emitter.fire({
            operationId: params.operationId,
            phase: "committing",
            uploadedBytes: totalBytes,
            totalBytes,
          });
          const commit = await deps.channel.attachmentCommitV4({ ...target, uploadId });
          ref = commit.ref;
        }
        record.committed = true;
        records.delete(params.operationId);
        emitter.fire({
          operationId: params.operationId,
          phase: "complete",
          uploadedBytes: totalBytes,
          totalBytes,
        });
        return { operationId: params.operationId, ref, bytes: totalBytes, staged: true };
      } catch (error) {
        if (!record.committed) {
          await abortRemote(target, uploadId);
        }
        if (records.get(params.operationId) === record) {
          records.delete(params.operationId);
        }
        if (isAbortLike(error) || record.controller.signal.aborted) {
          emitter.fire({
            operationId: params.operationId,
            phase: "canceled",
            uploadedBytes: 0,
            totalBytes: 0,
          });
          throw error;
        }
        const message = error instanceof Error ? error.message : String(error);
        if (message === "fault.attachment.putUnsupported") {
          throw new Error(LAN_PUT_UNSUPPORTED_MESSAGE, { cause: error });
        }
        // 本地确定性错误（超限/不是文件/不存在）原样上抛，保住可读文案与永久失败分类。
        if (
          message === LAN_STAGE_LIMIT_MESSAGE ||
          message.startsWith("附件不是文件") ||
          message.startsWith("附件文件不存在")
        ) {
          throw error;
        }
        throw new Error(`局域网附件上传失败：${params.fileName}`, { cause: error });
      }
    },
    async adopt() {
      // artifact store 对未引用条目按 attachmentUnreferencedTtlMs(24h) 自动回收；
      // 已提交的 LAN 附件无远端路径需要保留或删除。
    },
    async cancel(operationId) {
      await cancelInternal(operationId);
    },
    async cleanup(operationId) {
      await cancelInternal(operationId);
    },
    onDynamicProgress: (operationId) => getEmitter(operationId).event,
  };
}
```

- [ ] **Step 4: 运行夹具确认通过**

Run: `node --test packages/desktop/tests/lan-prompt-attachments.test.mts`
Expected: PASS（A-E 五段断言全过）

- [ ] **Step 5: 提交**

```bash
git add packages/desktop/src/host/lanPromptAttachments.ts packages/desktop/tests/lan-prompt-attachments.test.mts packages/desktop/tests/fixtures/lan-prompt-attachments.ts
git commit -m "feat(desktop): LAN stage 走附件 chunk 通道上传"
```

---

### Task 7: LAN materialize —— 内联转换与路径排序判定

**Files:**
- Modify: `packages/desktop/src/host/lanPromptAttachments.ts`（追加 materialize）
- Modify: `packages/desktop/src/host/remotePromptAttachments.ts`（`REMOTE_PROMPT_ATTACHMENT_ROOT` 加 `export`）
- Create: `packages/desktop/tests/fixtures/lan-materialize.ts`
- Create: `packages/desktop/tests/lan-materialize.test.mts`

**Interfaces:**
- Consumes: Task 5 导出的 `isPathInRoot`、`replaceContentPaths`；`REMOTE_PROMPT_ATTACHMENT_ROOT`（本任务加 export）；`ModePromptAttachment`、`TraceId`（`@mode/shared`）、`PROTOCOL_V4_LIMITS`、`IFileService`（`@mode/services`）
- Produces: `materializeLanPromptAttachments(input, options): Promise<{ content: string; attachments?: ModePromptAttachment[]; uploadedCount: number }>`，`options = { fileService: Pick<IFileService, "readdir">; readFileBase64?: (path) => Promise<{ dataBase64: string; bytes: number }> }`。Task 8 接线消费。

- [ ] **Step 1: 写失败夹具**

创建 `packages/desktop/tests/fixtures/lan-materialize.ts`：

```ts
/**
 * LAN materialize 夹具：内联放行 / 本机读取转内联（内容路径改写）/
 * 超限拒绝 / 本机缺失+目标机存在放行 / 两侧缺失报错。打印 LAN_MATERIALIZE_OK。
 */
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { materializeLanPromptAttachments } from "../../src/host/lanPromptAttachments.js";

async function main() {
  const dir = await mkdtemp(join(tmpdir(), "lan-mat-"));
  try {
    const localFile = join(dir, "doc.txt");
    await writeFile(localFile, "hello lan");

    const fileService = {
      async readdir(params: { path: string }) {
        if (params.path.replace(/\/gu, "/").endsWith("/remote")) {
          return [
            { name: "remote-only.bin", path: "/remote/remote-only.bin", type: "file" as const },
          ];
        }
        throw new Error(`ENOENT: ${params.path}`);
      },
    };

    // 1) dataBase64 内联 → 原样放行
    const inline = await materializeLanPromptAttachments(
      {
        taskId: "t",
        traceId: "r1",
        content: "hi",
        attachments: [
          { kind: "image", filename: "p.png", mimeType: "image/png", dataBase64: "QUJD" },
        ],
      },
      { fileService: fileService as never },
    );
    assert.equal(inline.attachments?.[0]?.dataBase64, "QUJD");

    // 2) 本机文件 → 转内联 dataBase64、去 localPath、内容路径改写为文件名
    const converted = await materializeLanPromptAttachments(
      {
        taskId: "t",
        traceId: "r2",
        content: `ref: ${localFile}`,
        attachments: [
          {
            kind: "file",
            filename: "doc.txt",
            mimeType: "text/plain",
            sizeBytes: 9,
            localPath: localFile,
          },
        ],
      },
      { fileService: fileService as never },
    );
    const first = converted.attachments?.[0];
    assert.ok(first && "dataBase64" in first && first.dataBase64, "必须转成内联");
    assert.equal(first.localPath, undefined, "localPath 必须移除（不变量 1）");
    assert.ok(!converted.content.includes(localFile), "内容中的本机路径不得抵达对端");
    assert.ok(converted.content.includes("doc.txt"));

    // 3) 超 20 MiB → 可读错误（注入 readFileBase64 模拟，避免真写 20 MiB）
    await assert.rejects(
      materializeLanPromptAttachments(
        {
          taskId: "t",
          traceId: "r3",
          content: "",
          attachments: [
            {
              kind: "file",
              filename: "big.bin",
              mimeType: "application/octet-stream",
              sizeBytes: 0,
              localPath: join(dir, "big.bin"),
            },
          ],
        },
        {
          fileService: fileService as never,
          readFileBase64: async () => {
            throw Object.assign(new Error("局域网附件单文件上限 20 MiB：big.bin"), {
              code: "LAN_INLINE_TOO_LARGE",
            });
          },
        },
      ),
      /局域网附件单文件上限/u,
    );

    // 4) 本机不存在、目标机存在 → 原样放行
    const pass = await materializeLanPromptAttachments(
      {
        taskId: "t",
        traceId: "r4",
        content: "",
        attachments: [
          {
            kind: "file",
            filename: "remote-only.bin",
            mimeType: "application/octet-stream",
            sizeBytes: 1,
            localPath: "/remote/remote-only.bin",
          },
        ],
      },
      { fileService: fileService as never },
    );
    assert.equal(pass.attachments?.[0]?.localPath, "/remote/remote-only.bin");

    // 5) 两侧都不存在 → 可读错误
    await assert.rejects(
      materializeLanPromptAttachments(
        {
          taskId: "t",
          traceId: "r5",
          content: "",
          attachments: [
            {
              kind: "file",
              filename: "x",
              mimeType: "text/plain",
              sizeBytes: 1,
              localPath: "/nowhere/x.txt",
            },
          ],
        },
        { fileService: fileService as never },
      ),
      /附件路径在本机和远端都不存在/u,
    );

    console.log("LAN_MATERIALIZE_OK");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

await main();
```

`packages/desktop/tests/lan-materialize.test.mts`：同 Task 5 wrapper 结构，test 名「LAN materialize：内联转换 / 超限拒绝 / 路径排序判定」，fixture `./fixtures/lan-materialize.ts`，marker `/LAN_MATERIALIZE_OK/`。

- [ ] **Step 2: 运行确认失败**

Run: `node --test packages/desktop/tests/lan-materialize.test.mts`
Expected: FAIL —— `materializeLanPromptAttachments` 未导出。

- [ ] **Step 3: 实现 LAN materialize**

1. `remotePromptAttachments.ts`：`const REMOTE_PROMPT_ATTACHMENT_ROOT = ...` 改 `export const`。
2. `lanPromptAttachments.ts` 追加（import 合并进既有块）：

```ts
import type { ModePromptAttachment, TraceId } from "@mode/shared";
import type { IFileService } from "@mode/services";
import {
  REMOTE_PROMPT_ATTACHMENT_ROOT,
  isPathInRoot,
  replaceContentPaths,
} from "./remotePromptAttachments.js";

export interface LanMaterializeInput {
  taskId: string;
  traceId: TraceId | string;
  content: string;
  attachments?: ModePromptAttachment[];
}

export interface LanMaterializeOptions {
  fileService: Pick<IFileService, "readdir">;
  /** 测试注入；缺省 = fs stat + 流式读 → base64（≤ attachmentMaxBytes）。 */
  readFileBase64?: (path: string) => Promise<{ dataBase64: string; bytes: number }>;
}

const LAN_INLINE_LIMIT_MARKER = "局域网附件单文件上限 20 MiB";

async function defaultReadFileBase64(path: string): Promise<{ dataBase64: string; bytes: number }> {
  const info = await stat(path);
  if (!info.isFile()) {
    throw Object.assign(new Error(`附件不是文件：${path}`), { code: "LAN_INLINE_NOT_FILE" });
  }
  if (info.size > PROTOCOL_V4_LIMITS.attachmentMaxBytes) {
    const base = path.split(/[\/]/u).at(-1) ?? path;
    throw Object.assign(new Error(`${LAN_INLINE_LIMIT_MARKER}：${base}`), {
      code: "LAN_INLINE_TOO_LARGE",
    });
  }
  const parts: Buffer[] = [];
  for await (const piece of createReadStream(path)) parts.push(piece as Buffer);
  return { dataBase64: Buffer.concat(parts).toString("base64"), bytes: info.size };
}

function isNotFoundLike(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return error.message.includes("ENOENT") || (error as { code?: string }).code === "ENOENT";
}

function isTooLargeLike(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return (
    error.message.includes(LAN_INLINE_LIMIT_MARKER) ||
    (error as { code?: string }).code === "LAN_INLINE_TOO_LARGE"
  );
}

function dirnameLikePath(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (index < 0) return ".";
  if (index === 0) return path.slice(0, 1);
  return path.slice(0, index);
}

function basenameLikePath(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return index >= 0 ? path.slice(index + 1) : path;
}

async function remoteFileExists(
  fileService: Pick<IFileService, "readdir">,
  path: string,
): Promise<boolean> {
  try {
    const entries = await fileService.readdir({ path: dirnameLikePath(path), includeHidden: true });
    const base = basenameLikePath(path);
    return entries.some(
      (entry) => entry.path === path || (entry.name === base && entry.type === "file"),
    );
  } catch {
    return false;
  }
}

export async function materializeLanPromptAttachments(
  input: LanMaterializeInput,
  options: LanMaterializeOptions,
): Promise<{ content: string; attachments?: ModePromptAttachment[]; uploadedCount: number }> {
  const attachments = input.attachments;
  if (!attachments || attachments.length === 0) {
    return { content: input.content, attachments: input.attachments, uploadedCount: 0 };
  }
  const replacements = new Map<string, string>();
  const nextAttachments: ModePromptAttachment[] = [];
  let uploadedCount = 0;
  let changed = false;

  for (const attachment of attachments) {
    const localPath = attachment.localPath?.trim();
    if (!localPath) {
      nextAttachments.push(attachment);
      continue;
    }
    if (isPathInRoot(localPath, REMOTE_PROMPT_ATTACHMENT_ROOT)) {
      // 与 SSH 对齐：staging 根路径已属于对端，透传。
      nextAttachments.push(attachment);
      continue;
    }
    try {
      const inline = await (options.readFileBase64 ?? defaultReadFileBase64)(localPath);
      const { localPath: _dropped, ...rest } = attachment;
      nextAttachments.push({ ...rest, dataBase64: inline.dataBase64 } as ModePromptAttachment);
      // 内容里引用本机路径的位置改写为文件名：主机路径永不抵达对端（不变量 1）。
      replacements.set(localPath, attachment.filename);
      uploadedCount += 1;
      changed = true;
    } catch (error) {
      if (isTooLargeLike(error)) throw error;
      if (!isNotFoundLike(error)) {
        throw new Error(`本地附件读取失败：${attachment.filename}`, { cause: error });
      }
      if (await remoteFileExists(options.fileService, localPath)) {
        // 遗留转换把对端路径落成 localPath：对端本就可读，原样放行。
        nextAttachments.push(attachment);
        continue;
      }
      throw new Error(`附件路径在本机和远端都不存在：${localPath}`);
    }
  }

  return {
    content: replaceContentPaths(input.content, replacements),
    attachments: changed ? nextAttachments : attachments,
    uploadedCount,
  };
}
```

- [ ] **Step 4: 运行夹具确认通过**

Run: `node --test packages/desktop/tests/lan-materialize.test.mts`
Expected: PASS（5 段断言）

- [ ] **Step 5: 提交**

```bash
git add packages/desktop/src/host/lanPromptAttachments.ts packages/desktop/src/host/remotePromptAttachments.ts packages/desktop/tests/
git commit -m "feat(desktop): LAN materialize 内联转换与路径排序判定"
```

---

### Task 8: LAN 句柄接线（拆掉硬拦截）+ 重试分类联动

**Files:**
- Modify: `packages/desktop/src/host/index.ts`（`createWindowLanRemoteConnectionHandle` 内 1630-1653 区段）
- Modify: `packages/ui/src/v4/composer/useComposerAttachments.ts`（`isTransientAttachmentUploadError` 负向正则）

**Interfaces:**
- Consumes: Task 6 `createLanPromptAttachmentTransferService`、Task 7 `materializeLanPromptAttachments`；`connection.services.modeAgentService` / `connection.services.fileService`（`attachLanRemoteConnection` 返回的 `IServiceAccessor`）
- Produces: LAN 连接的 renderer 可见行为：stage 可用、materialize 放行/转换（对 renderer 无新接口）。

- [ ] **Step 1: 替换 stub**

`packages/desktop/src/host/index.ts` 的 `createWindowLanRemoteConnectionHandle` 中，把 1630-1653 行（`unsupportedAttachmentError` 定义到 `promptAttachmentTransferService` stub 整段）替换为：

```ts
  const promptAttachmentTransferService = createLanPromptAttachmentTransferService({
    channel: connection.services.modeAgentService,
  });
  const materializePromptAttachments = async (request: {
    taskId: string;
    traceId: TraceId | string;
    content: string;
    attachments?: ModePromptAttachment[];
  }) => {
    const result = await materializeLanPromptAttachments(request, {
      fileService: connection.services.fileService,
    });
    return { content: result.content, attachments: result.attachments };
  };
```

import 增加：

```ts
import {
  createLanPromptAttachmentTransferService,
  materializeLanPromptAttachments,
} from "./lanPromptAttachments.js";
```

清理：删除 `unsupportedAttachmentError` 与 1646 行 `progressEmitter` 常量；`Emitter`、`PromptAttachmentTransferProgress`、`IPromptAttachmentTransferService` 等 import 以 grep 为准——仅删本文件内已无使用点的。`createRemoteWorkspaceServiceCollection` 的两个入参（`materializePromptAttachments`、`promptAttachmentTransferService`）保持原位不动。

- [ ] **Step 2: 重试分类联动**

`packages/ui/src/v4/composer/useComposerAttachments.ts` 的 `isTransientAttachmentUploadError`（约 169 行）负向正则追加永久失败标记（中文 host 错误不得触发自动重试）：

```ts
  return !/(?:payloadTooLarge|invalidBase64|invalidServerProgress|frameTooLarge|permission|EACCES|ENOENT|not found|unsupported|附件缺少|缺少可读取内容|远端附件未完成物化|局域网连接附件单文件上限|对端版本不支持附件上传|附件文件不存在)/iu.test(
    message,
  );
```

- [ ] **Step 3: 验证并提交**

Run: `pnpm typecheck && pnpm lint && node --test packages/desktop/tests/lan-prompt-attachments.test.mts packages/desktop/tests/lan-materialize.test.mts packages/desktop/tests/remote-prompt-attachments-order.test.mts`
Expected: 全 PASS

```bash
git add packages/desktop/src/host/index.ts packages/ui/src/v4/composer/useComposerAttachments.ts
git commit -m "feat(desktop): LAN 句柄接入附件传输与物化"
```

---

### Task 9: composer 远端选文件模型（workspace-pick）

**Files:**
- Modify: `packages/ui/src/lib/chatAttachments.ts`
- Modify: `packages/ui/src/v4/composer/useComposerAttachments.ts`
- Create: `packages/ui/test/composerWorkspacePickAttachment.test.ts`

**Interfaces:**
- Consumes: `createChatComposerPathAttachment`、`ChatComposerAttachment`（同包）；`isRemoteAttachmentTarget`（同文件私有，需加 `export`）
- Produces: `ChatComposerAttachment.workspacePick?: boolean`、`createWorkspacePickPathAttachment(path)`、`resolveComposerAttachmentTransferMode(attachment, target): "localZeroCopy" | "workspacePick" | "stage" | "upload"`（导出）、`ComposerAttachmentsApi.addWorkspacePickAttachments(paths: string[])`。Task 11 消费 `addWorkspacePickAttachments`。

- [ ] **Step 1: 写失败测试**

创建 `packages/ui/test/composerWorkspacePickAttachment.test.ts`：

```ts
/** 远端选文件附件模型（spec: remote-workspace-bidirectional-attachments 发向·远端选文件）。
 * 运行：pnpm exec tsx --test packages/ui/test/composerWorkspacePickAttachment.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createWorkspacePickPathAttachment } from "../src/lib/chatAttachments.js";
import { resolveComposerAttachmentTransferMode } from "../src/v4/composer/useComposerAttachments.js";

test("createWorkspacePickPathAttachment：workspacePick 标记 + 路径推导文件名", () => {
  const item = createWorkspacePickPathAttachment("D:/remote/proj/report.pdf");
  assert.equal(item.workspacePick, true);
  assert.equal(item.localPath, "D:/remote/proj/report.pdf");
  assert.equal(item.filename, "report.pdf");
});

test("转移模式矩阵：workspace-pick 跳过 stage，远端 localPath 走 stage，本地零拷贝，无路径走上传", () => {
  const remoteTarget = { remoteSessionId: "rs-1", workspaceIdentity: "remote:lan:h:p:D:/r" };
  const localTarget = { workspacePath: "D:/local" };
  const pick = { localPath: "/remote/a.txt", workspacePick: true };
  assert.equal(resolveComposerAttachmentTransferMode(pick, remoteTarget), "workspacePick");
  assert.equal(resolveComposerAttachmentTransferMode(pick, localTarget), "localZeroCopy");
  assert.equal(
    resolveComposerAttachmentTransferMode({ localPath: "/host/b.txt" }, remoteTarget),
    "stage",
  );
  assert.equal(
    resolveComposerAttachmentTransferMode({ localPath: "/host/b.txt" }, localTarget),
    "localZeroCopy",
  );
  assert.equal(resolveComposerAttachmentTransferMode({}, remoteTarget), "upload");
  // target 缺省按本地处理（fail-safe：不把无目标当远端）。
  assert.equal(
    resolveComposerAttachmentTransferMode({ localPath: "/host/b.txt" }, undefined),
    "localZeroCopy",
  );
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm exec tsx --test packages/ui/test/composerWorkspacePickAttachment.test.ts`
Expected: FAIL —— `createWorkspacePickPathAttachment` / `resolveComposerAttachmentTransferMode` 不存在。

- [ ] **Step 3: 实现模型**

`chatAttachments.ts`：

1. `ChatComposerAttachment` 接口（46 行）追加：

```ts
  /** 从远端工作区文件树选中：路径已在目标机，跳过 staging 直接作为引用。 */
  workspacePick?: boolean;
```

2. `createChatComposerPathAttachment` 之后追加：

```ts
export function createWorkspacePickPathAttachment(path: string): ChatComposerAttachment {
  return { ...createChatComposerPathAttachment(path), workspacePick: true };
}
```

`useComposerAttachments.ts`：

1. `isRemoteAttachmentTarget`（179 行）加 `export`。
2. 新增并导出：

```ts
export type ComposerAttachmentTransferMode = "localZeroCopy" | "workspacePick" | "stage" | "upload";

/** 单附件转移路径裁决：唯一判定点，addPrepared 状态与 runUpload 分支都从这里取。 */
export function resolveComposerAttachmentTransferMode(
  attachment: Pick<ChatComposerAttachment, "localPath" | "workspacePick">,
  target: { remoteSessionId?: string; workspaceIdentity?: string } | undefined,
): ComposerAttachmentTransferMode {
  if (!attachment.localPath) return "upload";
  if (attachment.workspacePick) {
    // 远端选文件只在远端 target 出现；本地 target 按普通路径零拷贝兜底。
    return isRemoteAttachmentTarget(target ?? {}) ? "workspacePick" : "localZeroCopy";
  }
  return isRemoteAttachmentTarget(target ?? {}) ? "stage" : "localZeroCopy";
}
```

3. `addPreparedAttachments`（约 640 行）中 `localZeroCopy` 计算替换为：

```ts
        const mode = resolveComposerAttachmentTransferMode(attachment, target);
        const noTransfer = mode === "localZeroCopy" || mode === "workspacePick";
        return {
          ...attachment,
          referenceOwnership: "composer",
          operationId: `prompt-attachment-${attachment.id}`,
          uploadStatus: noTransfer ? "ready" : target?.sessionId ? "queued" : "waitingSession",
          uploadProgress: noTransfer ? 100 : 0,
          ...(noTransfer && attachment.localPath
            ? {
                attachmentRef: {
                  ref: attachment.localPath,
                  fileName: attachment.filename,
                  mime: attachment.mimeType,
                  bytes: attachment.sizeBytes,
                },
              }
            : {}),
          autoRetryCount: 0,
          runtimeRebuildRetryCount: 0,
          staged: false,
          adopted: false,
          showComplete: false,
          localZeroCopy: noTransfer,
        };
```

（`localZeroCopy: noTransfer` 让 workspace-pick 复用既有「runtime 重启不作废」跳过——`invalidateAttachmentsForRuntimeChange` 540-546 行按 `item.localZeroCopy` continue。）

4. `runUpload` stage 分支（315 行）加护栏：

```ts
        if (item.localPath && !item.workspacePick && isRemoteAttachmentTarget(target)) {
```

5. 新 API：

```ts
  const addWorkspacePickAttachments = useCallback(
    (paths: string[]) => {
      addPreparedAttachments(paths.map(createWorkspacePickPathAttachment));
    },
    [addPreparedAttachments],
  );
```

`ComposerAttachmentsApi` 接口（76 行）加：

```ts
  /** 远端工作区文件树选中项：已在目标机，直接构造 ready ref。 */
  addWorkspacePickAttachments: (paths: string[]) => void;
```

返回对象与 `useMemo` 依赖数组同步加入；顶部 `@/lib/chatAttachments.js` import 增加 `createWorkspacePickPathAttachment`。

- [ ] **Step 4: 运行测试与门禁**

Run: `pnpm exec tsx --test packages/ui/test/composerWorkspacePickAttachment.test.ts && pnpm typecheck && pnpm lint`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add packages/ui/src/lib/chatAttachments.ts packages/ui/src/v4/composer/useComposerAttachments.ts packages/ui/test/composerWorkspacePickAttachment.test.ts
git commit -m "feat(ui): composer 远端选文件附件模型"
```

---

### Task 10: DirectoryBrowser 文件多选模式

**Files:**
- Modify: `packages/ui/src/DirectoryBrowser.tsx`
- Create: `packages/ui/test/directoryBrowserFileMode.test.ts`
- Modify: `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts`

**Interfaces:**
- Consumes: `FileEntry`（`@mode/shared`：`{name, path, type, isSymbolicLink?}`）、`services.fileService.readdir` / `services.systemService.info`
- Produces: `DirectoryBrowser` 新 props `mode?: "directory" | "file"`（默认 `"directory"`）、`initialPath?: string`、`onSelectFiles?: (paths: string[]) => void`；导出纯函数 `selectDirectoryBrowserEntries(entries, mode)`。Task 11 消费。

- [ ] **Step 1: 写失败测试**

创建 `packages/ui/test/directoryBrowserFileMode.test.ts`：

```ts
/** DirectoryBrowser file 模式过滤（spec: remote-workspace-bidirectional-attachments 远端选文件）。
 * 运行：pnpm exec tsx --test packages/ui/test/directoryBrowserFileMode.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { selectDirectoryBrowserEntries } from "../src/DirectoryBrowser.js";

const entries = [
  { name: "src", path: "/w/src", type: "directory" as const },
  { name: "a.txt", path: "/w/a.txt", type: "file" as const },
];

test("directory 模式只留目录（既有行为）", () => {
  assert.deepEqual(
    selectDirectoryBrowserEntries(entries, "directory").map((entry) => entry.name),
    ["src"],
  );
});

test("file 模式保留目录与文件", () => {
  assert.deepEqual(
    selectDirectoryBrowserEntries(entries, "file").map((entry) => entry.name),
    ["src", "a.txt"],
  );
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm exec tsx --test packages/ui/test/directoryBrowserFileMode.test.ts`
Expected: FAIL —— `selectDirectoryBrowserEntries` 未导出。

- [ ] **Step 3: 实现 file 模式**

`DirectoryBrowser.tsx`：

1. import 追加 `FileIcon`（lucide-react）、`cn`（`@/components/lib/utils.js`）。
2. 纯函数（`filterDirectoryBrowserEntries` 旁）：

```ts
export function selectDirectoryBrowserEntries(
  entries: FileEntry[],
  mode: "directory" | "file",
): FileEntry[] {
  return mode === "file" ? entries : filterDirectoryBrowserEntries(entries);
}
```

3. props 追加（含 JSDoc），默认 `mode = "directory"`：

```ts
  mode?: "directory" | "file";
  /** file 模式起点；缺省 homedir。 */
  initialPath?: string;
  /** file 模式多选确认回调。 */
  onSelectFiles?: (paths: string[]) => void;
```

4. 选择状态：

```ts
  const [selectedPaths, setSelectedPaths] = useState<ReadonlySet<string>>(() => new Set());
```

5. 初始化 effect 中 `navigateTo(info.homedir)` 改为 `navigateTo(initialPath?.trim() || info.homedir)`；`.catch` 分支在 `initialPath?.trim()` 有值时先尝试 `navigateTo(initialPath.trim())` 再 return（远端 systemService 抖动不阻断浏览），否则保持原错误路径。

6. `navigateTo` 内 `setEntries(filterDirectoryBrowserEntries(result))` → `setEntries(selectDirectoryBrowserEntries(result, mode))`（`mode` 组件生命周期不变，闭包直接引用；若 react-hooks lint 报 deps，用 `const modeRef = useRef(mode)` 取 ref）。

7. 空态：`id: entries.length === 0 && mode === "file" ? "directoryBrowser.noEntries" : "directoryBrowser.noSubdirs"`。

8. 条目渲染：`mode === "file" && entry.type === "file"` 的条目渲染为选择切换按钮，其余沿用既有目录导航按钮。完整 JSX：

```tsx
          entries.map((entry) => {
            if (mode === "file" && entry.type === "file") {
              const selected = selectedPaths.has(entry.path);
              return (
                <button
                  key={entry.path}
                  type="button"
                  aria-pressed={selected}
                  data-selected={selected || undefined}
                  onClick={() =>
                    setSelectedPaths((current) => {
                      const next = new Set(current);
                      if (next.has(entry.path)) next.delete(entry.path);
                      else next.add(entry.path);
                      return next;
                    })
                  }
                  className={cn(
                    "flex w-full shrink-0 cursor-pointer items-center gap-2 px-3 py-2 text-start text-ui-base transition hover:bg-hover/50",
                    selected && "bg-accent/60",
                  )}
                >
                  <FileIcon className="size-4 shrink-0 text-foreground-subtle" />
                  <span className="truncate">{entry.name}</span>
                </button>
              );
            }
            const EntryIcon =
              getDirectoryBrowserEntryIconKind(entry) === "folder-symlink"
                ? FolderSymlinkIcon
                : FolderIcon;
            return (
              <button
                key={entry.path}
                onClick={() => navigateTo(entry.path)}
                className="flex w-full shrink-0 cursor-pointer items-center gap-2 px-3 py-2 text-start text-ui-base transition hover:bg-hover/50"
              >
                <EntryIcon className="size-4 shrink-0 text-foreground-subtle" />
                <span className="truncate">{entry.name}</span>
              </button>
            );
          })
```

9. 非嵌入 footer 主按钮按模式分流：

```tsx
          {mode === "file" ? (
            <Button
              type="button"
              size="lg"
              className="h-10 min-w-0 px-5 text-ui-base"
              disabled={selectedPaths.size === 0 || loading}
              onClick={() => onSelectFiles?.([...selectedPaths])}
            >
              {intl.formatMessage(
                { id: "directoryBrowser.selectFiles" },
                { count: String(selectedPaths.size) },
              )}
            </Button>
          ) : (
            <Button
              type="button"
              size="lg"
              className="h-10 min-w-0 px-5 text-ui-base"
              disabled={!currentPath.trim() || loading}
              onClick={handleSelectCurrentPath}
            >
              {intl.formatMessage({ id: "directoryBrowser.selectDir" })}
            </Button>
          )}
```

10. 三个 locale 追加：

| key | zh-CN | en-US | fa |
| --- | --- | --- | --- |
| `directoryBrowser.selectFiles` | `选择附件（{count}）` | `Attach ({count})` | `پیوست ({count})` |
| `directoryBrowser.noEntries` | `无文件` | `No files` | `بدون فایل` |

- [ ] **Step 4: 运行测试与门禁**

Run: `pnpm exec tsx --test packages/ui/test/directoryBrowserFileMode.test.ts && pnpm typecheck && pnpm lint`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add packages/ui/src/DirectoryBrowser.tsx packages/ui/test/directoryBrowserFileMode.test.ts packages/ui/src/i18n/locales/
git commit -m "feat(ui): DirectoryBrowser 文件多选模式"
```

---

### Task 11: composer「从工作区选择文件」入口接线

**Files:**
- Modify: `packages/ui/src/prompt-editor/ChatPromptActionMenu.tsx`
- Modify: `packages/ui/src/prompt-editor/ChatPromptEditor.tsx`
- Modify: `packages/ui/src/v4/ConversationComposer.tsx`
- Modify: `packages/shared/src/test-ids.ts`
- Modify: `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts`

**Interfaces:**
- Consumes: Task 9 `attachmentsApi.addWorkspacePickAttachments`、`isRemoteAttachmentTarget`；Task 10 `DirectoryBrowser` 新 props；`useWorkspaceServicesResolution`（`@/hooks/useWorkspaceServices.js`）
- Produces: `ChatPromptActionMenu`/`ChatPromptEditor` prop 由 `attachmentAction?: {...}` 改为 `attachmentActions?: ReadonlyArray<{label: string; onSelect: () => void; testId?: string; menuItemTestId?: string}>`（唯一调用链 ConversationComposer → ChatPromptEditor → ChatPromptActionMenu；改名先 `grep -rn "attachmentAction" packages/ui/src` 确认无其他调用方）。

- [ ] **Step 1: TID 与 i18n**

`packages/shared/src/test-ids.ts`（`TID_CHAT_ATTACHMENT_MENU_ITEM` 旁）追加：

```ts
export const TID_CHAT_WORKSPACE_PICK_MENU_ITEM = "chat-workspace-pick-menu-item";
export const TID_CHAT_WORKSPACE_PICK_BUTTON = "chat-workspace-pick-button";
```

三个 locale 追加：

| key | zh-CN | en-US | fa |
| --- | --- | --- | --- |
| `chat.composer.attachWorkspaceFile` | `从工作区选择文件` | `Select from workspace` | `انتخاب فایل از فضای کاری` |

- [ ] **Step 2: 菜单 prop 数组化**

`ChatPromptActionMenu.tsx`：

1. 解构 `attachmentAction,` → `attachmentActions,`；props 类型：

```ts
  attachmentActions?: ReadonlyArray<{
    label: string;
    onSelect: () => void;
    testId?: string;
    menuItemTestId?: string;
  }>;
```

2. `const attachmentCount = attachmentAction ? 1 : 0;` → `const attachmentCount = attachmentActions?.length ?? 0;`
3. options：`...(attachmentAction ? [{ disabled: false }] : [])` → `...((attachmentActions ?? []).map(() => ({ disabled: false })))`
4. sections 的 `add` 组：`...(attachmentAction ? [ ... ] : [])` → `...((attachmentActions ?? []).map((action, actionIndex) => ({ id: \`attach-${actionIndex}\`, label: action.label, description: "", content: (<> PaperclipIcon + span(data-testid={action.menuItemTestId}) {action.label} </>) })))`（content 沿用原结构，仅数据源换成 action）
5. `selectOption` 开头：

```ts
    if (attachmentActions && index < attachmentActions.length) {
      attachmentActions[index].onSelect();
      return;
    }
```

（后续 `quickCommands[index - attachmentCount]` 算术不变。）

`ChatPromptEditor.tsx`：65/116/178/395 四处 `attachmentAction` → `attachmentActions`；`hasActionMenu = Boolean(attachmentActions?.length) || showMentionButton || showSlashButton`；类型同上。

- [ ] **Step 3: ConversationComposer 接线**

`ConversationComposer.tsx`：

1. import 追加：`DirectoryBrowser`（`@/DirectoryBrowser.js`）、`useWorkspaceServicesResolution`（`@/hooks/useWorkspaceServices.js`）、`isRemoteAttachmentTarget`（`@/v4/composer/useComposerAttachments.js`）、`TID_CHAT_WORKSPACE_PICK_MENU_ITEM`/`TID_CHAT_WORKSPACE_PICK_BUTTON`（`@mode/shared`）。
2. 状态与服务（`attachmentAction` useMemo 之前）：

```ts
  const [workspacePickerOpen, setWorkspacePickerOpen] = useState(false);
  const { services: workspacePickerServices } = useWorkspaceServicesResolution(
    workspacePath,
    remoteSessionId,
    workspaceIdentity,
  );
```

3. 现有 `attachmentAction` useMemo（1477 行）替换为：

```ts
  const attachmentActions = useMemo(() => {
    const actions = [
      {
        label: intl.formatMessage({ id: "chat.composer.attachment" }),
        menuItemTestId: TID_CHAT_ATTACHMENT_MENU_ITEM,
        onSelect: () => attachmentsApi.openAttachmentPicker(),
        testId: TID_CHAT_ATTACHMENT_BUTTON,
      },
    ];
    if (!isRemoteAttachmentTarget({ remoteSessionId, workspaceIdentity })) return actions;
    return [
      ...actions,
      {
        label: intl.formatMessage({ id: "chat.composer.attachWorkspaceFile" }),
        menuItemTestId: TID_CHAT_WORKSPACE_PICK_MENU_ITEM,
        testId: TID_CHAT_WORKSPACE_PICK_BUTTON,
        onSelect: () => setWorkspacePickerOpen(true),
      },
    ];
  }, [attachmentsApi.openAttachmentPicker, intl, remoteSessionId, workspaceIdentity]);
```

4. 传参 `attachmentAction={attachmentAction}` → `attachmentActions={attachmentActions}`（grep `attachmentAction=` 定位到 ChatPromptEditor 调用点）。
5. 组件 return JSX 末尾（与其它 fixed 层并列）挂载：

```tsx
      {workspacePickerOpen ? (
        <DirectoryBrowser
          services={workspacePickerServices}
          mode="file"
          initialPath={workspacePath}
          onSelectFiles={(paths) => {
            attachmentsApi.addWorkspacePickAttachments(paths);
            setWorkspacePickerOpen(false);
          }}
          onCancel={() => setWorkspacePickerOpen(false)}
        />
      ) : null}
```

- [ ] **Step 4: 验证并提交**

Run: `pnpm typecheck && pnpm lint && pnpm exec tsx --test packages/ui/test/*.test.ts`
Expected: PASS（全量 ui 测试含本计划新增五件）

```bash
git add packages/ui/src/prompt-editor packages/ui/src/v4/ConversationComposer.tsx packages/shared/src/test-ids.ts packages/ui/src/i18n/locales/
git commit -m "feat(ui): composer 工作区文件选择入口"
```

---

### Task 12: 全量验证与 E2E 场景核对

**Files:**
- 无新增（验证 + 记录）

**Interfaces:**
- Consumes: 全部前置 Task
- Produces: 真实验证结果记录（附到最终汇报）

- [ ] **Step 1: 架构检查与全量门禁**

Run（逐条执行，如实记录输出）：

```bash
node scripts/check-workspace-freshness.mjs
pnpm architecture:check --changed
pnpm typecheck
pnpm lint
pnpm fmt:check
```

Expected: typecheck、lint 必须真实通过；若出现本计划引入的失败，修复后重跑；既有/环境失败单独列出，不冒充通过。

- [ ] **Step 2: 全部测试入口**

```bash
pnpm --filter @mode/bootstrap test
pnpm exec tsx --test packages/ui/test/*.test.ts
node --test packages/desktop/tests/lan-prompt-attachments.test.mts
node --test packages/desktop/tests/lan-materialize.test.mts
node --test packages/desktop/tests/remote-prompt-attachments-order.test.mts
```

Expected: 全 PASS。

- [ ] **Step 3: E2E 场景核对（环境受限，如实记录）**

仓库现状**没有**可运行的 E2E runner（无 playwright 配置；`harness/orca-evidence` 仅设置页取证；e2e bridge 无脚本接线）——spec 中「E2E（交互改动必需）」在本环境无法自动化执行。按 spec 验收场景逐条核对，能本地手动验证的用 `pnpm dev:desktop` 走一遍，无法验证的标注原因：

| # | 场景 | 方式 | 结果 |
| --- | --- | --- | --- |
| 1 | SSH 远端对话发 图片/PDF/txt → 预览 + 下载 | 手动（需 SSH 对端）/ 暂缓 | |
| 2 | LAN 远端对话同上 | 手动（需 LAN 对端）/ 暂缓 | |
| 3 | LAN 本地路径附件 ≤20 MiB 发送；>20 MiB 拒发带文案 | 单测（Task 6 C 段）+ 手动 | |
| 4 | LAN 粘贴截图发送不被 guard 拦 | 单测（Task 7 #1 段）+ 手动 | |
| 5 | 远端选文件入口 → 多选 → 发送可读；选目录不可提交 | 单测（Task 9/10）+ 手动 | |
| 6 | materialize 排序判定 | 单测（Task 5/7） | |
| 7 | artifact 行下载；图片类预览 | 单测（Task 1 授权）+ 手动 | |
| 8 | 超限下载提示 | 单测（Task 2 分类）+ 手动 | |
| 9 | 跨 session / ref 不匹配拒绝 | 单测（Task 1） | |
| 10 | 本地对话下载同样生效 | 手动 | |
| 11 | LAN stage 中断 failed 可重试；无 put hook 明确报错 | 单测（Task 6 D/E 段） | |

- [ ] **Step 4: 提交记录**

有结果记录文件则一并提交；无则跳过本步（不制造空提交）。

---

## Self-Review 记录

- **Spec 覆盖**：LAN stage（Task 6/8）、LAN materialize + 排序判定（Task 5/7/8）、远端选文件（Task 9/10/11）、收向授权 + artifact 行（Task 1）、下载四入口（Task 2/3/4，ImagePreviewDialog 已有下载不动）、错误矩阵（Task 2 分类 + Task 6 错误文案 + Task 8 重试分类）、i18n 三语（各 UI Task 内联表）、限额零改动（Global Constraints）、E2E 场景（Task 12 如实记录）。spec 11 条验收场景全部映射到 Task 12 核对表。
- **占位符扫描**：无 TBD/TODO；代码步骤均给出完整实现；Task 3 Step 3/4 的对话框调用点给出完整 props 模板与定位说明。
- **类型一致性**：`authorizeConversationAttachment` 签名在 Task 1 定义与 Step 5 接线一致；`resolveComposerAttachmentTransferMode` 字面量在 Task 9 测试与实现一致；`buildAttachmentDownloadReadParams` 的 `includeAttachmentIndex` 在 Task 2 测试与 Task 4 artifact 调用一致；`LanAttachmentChannel` 在 Task 6 定义、Task 8 消费一致；`materializeLanPromptAttachments` 依赖项 `{fileService}` 与 Task 8 接线一致；`suggestedAttachmentDownloadName` 控制字符用码点比较（不携带原始控制字节）。
