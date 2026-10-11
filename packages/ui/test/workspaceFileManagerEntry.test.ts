/** 远端工作区右上角文件管理器入口可见性（spec: remote-workspace-file-manager-entry）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/workspaceFileManagerEntry.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import type { LanConnectOptions, RemoteTarget } from "@mode/shared";
import { shouldShowWorkspaceFileManager } from "../src/lib/workspaceFileManagerEntry.js";

const lanTarget: LanConnectOptions = {
  kind: "lan",
  host: "192.168.1.20",
  port: 45880,
  serverId: "server-abc",
  serverName: "书房电脑",
  token: "secret-token",
};

test("本地工作区不显示入口：三种远端标识全空", () => {
  assert.equal(shouldShowWorkspaceFileManager({}), false);
  assert.equal(
    shouldShowWorkspaceFileManager({ workspaceIdentity: "   ", remoteSessionId: "" }),
    false,
  );
});

test("远端三种标识任一为真即显示", () => {
  assert.equal(shouldShowWorkspaceFileManager({ remoteTarget: lanTarget as RemoteTarget }), true);
  assert.equal(shouldShowWorkspaceFileManager({ workspaceIdentity: "lan:192.168.1.20" }), true);
  assert.equal(shouldShowWorkspaceFileManager({ remoteSessionId: "sess_1" }), true);
  // identity 带空白时按 trim 判定，与 workspaceIdentity?.trim() 的身份 key 口径一致。
  assert.equal(shouldShowWorkspaceFileManager({ workspaceIdentity: "  lan:192.168.1.20  " }), true);
});

test("窄屏远控抑制入口（simplifyForNarrowRemote 优先于远端标识）", () => {
  assert.equal(
    shouldShowWorkspaceFileManager({
      workspaceIdentity: "lan:192.168.1.20",
      simplifyForNarrowRemote: true,
    }),
    false,
  );
  assert.equal(
    shouldShowWorkspaceFileManager({ remoteSessionId: "sess_1", simplifyForNarrowRemote: true }),
    false,
  );
});
