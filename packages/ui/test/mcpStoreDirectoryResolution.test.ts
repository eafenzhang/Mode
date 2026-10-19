/**
 * MCP 本地目录读取的路径选择：Electron 保留平台路径，Tauri 改用 host 服务。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { shouldUseHostMcpDirectory } from "../src/store/mcpStore.js";

test("Tauri 本地 workspace 使用 host MCP 目录服务", () => {
  assert.equal(
    shouldUseHostMcpDirectory({
      platformSupportsLocalMcpDirectory: false,
      hasWorkspaceIdentity: false,
      hasDirectoryService: true,
    }),
    true,
  );
});

test("Electron 本地 workspace 保持平台路径（含 legacy 迁移）", () => {
  assert.equal(
    shouldUseHostMcpDirectory({
      platformSupportsLocalMcpDirectory: true,
      hasWorkspaceIdentity: false,
      hasDirectoryService: true,
    }),
    false,
  );
  assert.equal(
    shouldUseHostMcpDirectory({
      hasWorkspaceIdentity: false,
      hasDirectoryService: true,
    }),
    false,
  );
});

test("远端 workspace 始终使用目录服务；没有服务时不用", () => {
  assert.equal(
    shouldUseHostMcpDirectory({
      platformSupportsLocalMcpDirectory: false,
      hasWorkspaceIdentity: true,
      hasDirectoryService: true,
    }),
    true,
  );
  assert.equal(
    shouldUseHostMcpDirectory({
      platformSupportsLocalMcpDirectory: false,
      hasWorkspaceIdentity: false,
      hasDirectoryService: false,
    }),
    false,
  );
});
