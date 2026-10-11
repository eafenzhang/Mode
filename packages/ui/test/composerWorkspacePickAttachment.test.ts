/** 远端选文件附件模型（spec: remote-workspace-bidirectional-attachments 发向·远端选文件）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/composerWorkspacePickAttachment.test.ts
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
