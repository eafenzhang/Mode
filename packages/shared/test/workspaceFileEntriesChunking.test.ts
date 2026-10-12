import assert from "node:assert/strict";
import test from "node:test";
import {
  WORKSPACE_FILE_ENTRIES_CHUNK_SIZE,
  packWorkspaceFileEntries,
  unpackWorkspaceFileEntries,
} from "../src/workspaceFileEntriesCodec.js";
import { PROTOCOL_V4_LIMITS } from "../src/mode-protocol-v4/core.js";

// spec: docs/specs/remote-workspace-rpc-bulk-chunking.md
// 远端工作区全部 RPC 共享一条单连接 FIFO 通道，大帧会阻塞其后的小帧（队头阻塞）；
// 文件索引分块必须与协议自身的批量块口径一致，防止重回 MB 级单帧。

test("文件索引块大小不超过协议批量块口径（attachmentChunkMaxBytes）", () => {
  assert.ok(
    WORKSPACE_FILE_ENTRIES_CHUNK_SIZE <= PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes,
    `chunk=${WORKSPACE_FILE_ENTRIES_CHUNK_SIZE} 超过协议批量块 ${PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes}`,
  );
  assert.ok(WORKSPACE_FILE_ENTRIES_CHUNK_SIZE > 0);
});

test("packed 往返：目录/文件类型、CJK 与转义字符逐项相等（Windows 根）", () => {
  const rootPath = "D:\\gas-hub-xp";
  const entries = [
    { name: "src", path: "D:\\gas-hub-xp\\src", relativePath: "src", type: "directory" as const },
    {
      name: "index.ts",
      path: "D:\\gas-hub-xp\\src\\index.ts",
      relativePath: "src/index.ts",
      type: "file" as const,
    },
    {
      name: "含 空格? 与带反斜杠\\文件.ts",
      path: "D:\\gas-hub-xp\\目录\\含 空格? 与带反斜杠\\文件.ts",
      relativePath: "目录/含 空格? 与带反斜杠\\文件.ts",
      type: "file" as const,
    },
  ];
  const packed = packWorkspaceFileEntries(entries);
  const unpacked = unpackWorkspaceFileEntries(packed, rootPath);
  assert.deepEqual(unpacked, entries, "解包必须逐字段还原（含转义与分隔符回拼）");
});

test("packed 往返：posix 根保持 / 分隔符", () => {
  const entries = [
    { name: "app", path: "/home/dev/proj/app", relativePath: "app", type: "directory" as const },
    {
      name: "main.go",
      path: "/home/dev/proj/app/main.go",
      relativePath: "app/main.go",
      type: "file" as const,
    },
  ];
  const unpacked = unpackWorkspaceFileEntries(
    packWorkspaceFileEntries(entries),
    "/home/dev/proj",
  );
  assert.deepEqual(unpacked, entries);
});

test("packed 根路径收尾分隔符两种形态都兼容；空 packed 解包为空数组", () => {
  const entries = [
    {
      name: "a.txt",
      path: "D:\\ws\\a.txt",
      relativePath: "a.txt",
      type: "file" as const,
    },
  ];
  const packed = packWorkspaceFileEntries(entries);
  assert.deepEqual(unpackWorkspaceFileEntries(packed, "D:\\ws\\"), entries);
  assert.deepEqual(unpackWorkspaceFileEntries(packed, "D:\\ws"), entries);
  assert.deepEqual(unpackWorkspaceFileEntries("", "D:\\ws"), []);
});

test("按块大小切分后逐块解包：拼接结果与整体解包一致", () => {
  // 模拟 fetchWorkspaceFileEntriesPacked 的拉取循环：按 offset/length 切片，
  // 拼回 packed 后解包，必须与一次性解包逐项相等（跨块行不丢不重）。
  const rootPath = "D:\\ws";
  const entries = Array.from({ length: 5_000 }, (_, index) => {
    const relativePath = `src/mod-${index % 17}/file-${index}.ts`;
    return {
      name: `file-${index}.ts`,
      path: `D:\\ws\\${relativePath}`,
      relativePath,
      type: "file" as const,
    };
  });
  const packed = packWorkspaceFileEntries(entries);
  let assembled = "";
  for (let offset = 0; offset < packed.length; offset += WORKSPACE_FILE_ENTRIES_CHUNK_SIZE) {
    assembled += packed.slice(offset, offset + WORKSPACE_FILE_ENTRIES_CHUNK_SIZE);
  }
  assert.equal(assembled, packed, "分块拼接必须与原文逐字符相等");
  assert.deepEqual(
    unpackWorkspaceFileEntries(assembled, rootPath),
    unpackWorkspaceFileEntries(packed, rootPath),
  );
});
