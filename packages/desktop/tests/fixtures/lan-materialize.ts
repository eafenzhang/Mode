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
        if (params.path.replace(/\\/gu, "/").endsWith("/remote")) {
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
