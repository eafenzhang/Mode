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
  // 与 server StdioStream 契约一致：订阅点是 onClose(Event<number>)，不是 on("close")。
  onClose(cb: (code: number) => void): { dispose(): void };
};

function streamResult(code: number, stdout = ""): BackendStream {
  return {
    stdout: {
      on(_event, cb) {
        if (stdout) cb(stdout);
      },
    },
    stderr: { on: () => undefined },
    onClose(cb) {
      cb(code);
      return { dispose: () => undefined };
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
          // 真实 backend 的路径经 quotePosixPathArg 单引号包裹，先剥壳再比对清单。
          const normalized = path.startsWith("'") && path.endsWith("'") ? path.slice(1, -1) : path;
          return streamResult(remoteOnlyPaths.has(normalized) ? 0 : 1);
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
