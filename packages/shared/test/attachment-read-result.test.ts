/**
 * 分段读响应 schema 的「任意 MIME」回归（spec: remote-workspace-bidirectional-attachments 收向）。
 * 运行：pnpm --filter @mode/shared test
 *
 * 背景（2026-10 远端工作区附件下载失败）：读块响应曾按 image/video/pdf refine mediaType，
 * 而预览与下载共用这条 schema，txt/zip/exe（application/octet-stream）回读在客户端响应
 * 校验即抛 ZodError，用户只能看到通用失败。媒体 gate 只属于 attachmentPreviewSource，
 * 本文件同时钉住它不回归。
 */
import assert from "node:assert/strict";
import test from "node:test";
import { PROTOCOL_V4_LIMITS } from "../src/mode-protocol-v4/core.js";
import {
  v4AttachmentPreviewSourceResultSchema,
  v4AttachmentReadResultSchema,
} from "../src/mode-protocol-v4/transport.js";

/** 合法读块的最小形状；totalBytes 与 dataBase64 解耦，便于单点构造边界用例。 */
function readChunk(overrides: {
  mediaType?: string;
  dataBase64?: string;
  totalBytes?: number;
  nextOffset?: number | null;
}) {
  return {
    dataBase64: overrides.dataBase64 ?? Buffer.from("hello").toString("base64"),
    mediaType: overrides.mediaType ?? "text/plain",
    totalBytes: overrides.totalBytes ?? 5,
    nextOffset: overrides.nextOffset ?? null,
  };
}

test("读响应接受非媒体 MIME：txt / zip / octet-stream / 空 mime", () => {
  const mimes = [
    "text/plain",
    "application/zip",
    "application/octet-stream",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    // 权威行 mime 为空的历史行会回落成空串：MIME 不是授权因子，不能因此拦下整块读取。
    "",
  ];
  for (const mediaType of mimes) {
    const parsed = v4AttachmentReadResultSchema.safeParse(readChunk({ mediaType }));
    assert.equal(parsed.success, true, `mediaType=${JSON.stringify(mediaType)} 应通过读响应校验`);
  }
});

test("读响应仍接受媒体 MIME（image/video/pdf，含带参数 pdf）", () => {
  for (const mediaType of [
    "image/png",
    "video/mp4",
    "application/pdf",
    "Application/PDF; charset=binary",
  ]) {
    assert.equal(
      v4AttachmentReadResultSchema.safeParse(readChunk({ mediaType })).success,
      true,
      mediaType,
    );
  }
});

test("读响应结构/容量校验不回归：非法 base64、块超限、越界 nextOffset、totalBytes 超上界", () => {
  const cases: Array<[string, ReturnType<typeof readChunk>]> = [
    ["非法 base64", readChunk({ dataBase64: "不是 base64!!" })],
    [
      "块解码超 attachmentChunkMaxBytes",
      readChunk({
        dataBase64: Buffer.alloc(PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes + 1).toString("base64"),
      }),
    ],
    ["nextOffset 超过 totalBytes", readChunk({ nextOffset: 6 })],
    [
      "totalBytes 超 attachmentPreviewMaxBytes 线缆上界",
      readChunk({ totalBytes: PROTOCOL_V4_LIMITS.attachmentPreviewMaxBytes + 1 }),
    ],
  ];
  for (const [label, value] of cases) {
    assert.equal(v4AttachmentReadResultSchema.safeParse(value).success, false, label);
  }
});

test("非视频 20 MiB 上限由网关裁量：schema 不把超限响应打成 raw ZodError", () => {
  // gateway 对非视频按 attachmentMaxBytes 报稳定码 previewTooLarge（→「超容量」文案）；
  // 客户端 schema 若再拒一次，确定分类会降级成 unknown 通用失败（与 stat 上限同源的历史教训）。
  const parsed = v4AttachmentReadResultSchema.safeParse(
    readChunk({
      mediaType: "application/octet-stream",
      totalBytes: PROTOCOL_V4_LIMITS.attachmentMaxBytes + 1,
    }),
  );
  assert.equal(parsed.success, true);
});

test("image 超过非视频读上限 20 MiB 仍在 schema 拒绝（容量镜像不回归）", () => {
  const parsed = v4AttachmentReadResultSchema.safeParse(
    readChunk({ mediaType: "image/png", totalBytes: PROTOCOL_V4_LIMITS.attachmentMaxBytes + 1 }),
  );
  assert.equal(parsed.success, false);
});

test("attachmentPreviewSource 媒体 gate 不回归：local_path 只认 video", () => {
  assert.equal(v4AttachmentPreviewSourceResultSchema.safeParse({ kind: "chunked" }).success, true);
  assert.equal(
    v4AttachmentPreviewSourceResultSchema.safeParse({
      kind: "local_path",
      path: "/tmp/a.mp4",
      mediaType: "video/mp4",
    }).success,
    true,
  );
  assert.equal(
    v4AttachmentPreviewSourceResultSchema.safeParse({
      kind: "local_path",
      path: "/tmp/a.txt",
      mediaType: "text/plain",
    }).success,
    false,
  );
  assert.equal(
    v4AttachmentPreviewSourceResultSchema.safeParse({
      kind: "local_path",
      path: "/tmp/a.png",
      mediaType: "image/png",
    }).success,
    false,
  );
});
