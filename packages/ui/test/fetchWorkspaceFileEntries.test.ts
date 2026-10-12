/** 文件索引分块拉取循环（spec: remote-workspace-rpc-bulk-chunking 验收 2）。
 * 运行：pnpm exec tsx --test packages/ui/test/fetchWorkspaceFileEntries.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { WORKSPACE_FILE_ENTRIES_CHUNK_SIZE } from "@mode/shared/workspaceFileEntriesCodec";
import { fetchWorkspaceFileEntriesPacked } from "../src/workspace-file-search/fetchWorkspaceFileEntries.js";

interface RangeCall {
  rootPath: string;
  offset: number;
  length: number;
}

function createFakeFileService(total: string) {
  const rangeCalls: RangeCall[] = [];
  const lengthCalls: string[] = [];
  return {
    rangeCalls,
    lengthCalls,
    service: {
      async listWorkspaceFilesLength(params: { rootPath: string }): Promise<number> {
        lengthCalls.push(params.rootPath);
        return total.length;
      },
      async listWorkspaceFilesRange(params: {
        rootPath: string;
        offset: number;
        length: number;
      }): Promise<string> {
        rangeCalls.push({ ...params });
        return total.slice(params.offset, params.offset + params.length);
      },
    },
  };
}

test("按块大小逐段请求：首块 offset=0、步进=块大小、末块截断，拼装与原文相等", async () => {
  const total = "x".repeat(WORKSPACE_FILE_ENTRIES_CHUNK_SIZE * 2 + 123);
  const fake = createFakeFileService(total);

  const packed = await fetchWorkspaceFileEntriesPacked(fake.service, "D:\\ws");

  assert.equal(packed, total, "拼装结果必须与原文逐字符相等");
  assert.deepEqual(fake.lengthCalls, ["D:\\ws"]);
  assert.equal(fake.rangeCalls.length, 3, "两整块 + 一个截断尾块");
  assert.deepEqual(
    fake.rangeCalls.map((call) => [call.offset, call.length]),
    [
      [0, WORKSPACE_FILE_ENTRIES_CHUNK_SIZE],
      [WORKSPACE_FILE_ENTRIES_CHUNK_SIZE, WORKSPACE_FILE_ENTRIES_CHUNK_SIZE],
      [WORKSPACE_FILE_ENTRIES_CHUNK_SIZE * 2, WORKSPACE_FILE_ENTRIES_CHUNK_SIZE],
    ],
    "每次请求都携带全量块长，由服务端按剩余长度截断",
  );
  for (const call of fake.rangeCalls) {
    assert.equal(call.rootPath, "D:\\ws");
  }
});

test("空索引不发 range 请求", async () => {
  const fake = createFakeFileService("");
  const packed = await fetchWorkspaceFileEntriesPacked(fake.service, "D:\\ws");
  assert.equal(packed, "");
  assert.equal(fake.rangeCalls.length, 0);
});

test("恰好一块的索引只发一次 range 请求", async () => {
  const total = "y".repeat(WORKSPACE_FILE_ENTRIES_CHUNK_SIZE);
  const fake = createFakeFileService(total);
  const packed = await fetchWorkspaceFileEntriesPacked(fake.service, "D:\\ws");
  assert.equal(packed, total);
  assert.equal(fake.rangeCalls.length, 1);
});

test("块请求失败时整体 reject，不返回残缺 packed", async () => {
  const total = "z".repeat(WORKSPACE_FILE_ENTRIES_CHUNK_SIZE * 3);
  const fake = createFakeFileService(total);
  const failing = {
    async listWorkspaceFilesLength(params: { rootPath: string }): Promise<number> {
      return fake.service.listWorkspaceFilesLength(params);
    },
    async listWorkspaceFilesRange(params: {
      rootPath: string;
      offset: number;
      length: number;
    }): Promise<string> {
      if (params.offset === WORKSPACE_FILE_ENTRIES_CHUNK_SIZE) {
        throw new Error("fault.file.rangeFailed");
      }
      return fake.service.listWorkspaceFilesRange(params);
    },
  };

  await assert.rejects(
    () => fetchWorkspaceFileEntriesPacked(failing, "D:\\ws"),
    /fault\.file\.rangeFailed/,
  );
});
