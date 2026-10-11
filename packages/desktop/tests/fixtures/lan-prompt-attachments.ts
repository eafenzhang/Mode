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
