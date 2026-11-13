/** 工作区远端目标 → 本机编辑器目标收敛（spec: remote-workspace-file-manager-entry）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/workspaceOpenInEditorTarget.test.ts
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { RemoteTarget } from "@mode/shared";
import type { WindowTabState, WorkspaceTabState } from "../src/store/tabStore.js";
import { resolveWorkspaceOpenInEditorTarget } from "../src/lib/workspaceOpenInEditorTarget.js";

const WORKSPACE_PATH = "/home/me/project";

function workspaceTab(options: {
  id: string;
  remoteTarget?: RemoteTarget;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}): WorkspaceTabState {
  return {
    id: options.id,
    kind: "workspace",
    workspacePath: WORKSPACE_PATH,
    label: "project",
    remoteTarget: options.remoteTarget,
    workspaceIdentity: options.workspaceIdentity,
    remoteSessionId: options.remoteSessionId,
  };
}

const v3Target: RemoteTarget = {
  kind: "v3",
  deviceId: "019a0000-0000-7000-8000-000000000099",
  deviceName: "书房主机",
};

test("v3 工作区：保持远端身份，但不产出编辑器目标，且不抛错", () => {
  const workspaceIdentity = "remote:v3:019a0000-0000-7000-8000-000000000099:/home/me/project";
  const result = resolveWorkspaceOpenInEditorTarget(
    [workspaceTab({ id: "t1", remoteTarget: v3Target, workspaceIdentity })],
    { workspacePath: WORKSPACE_PATH, workspaceIdentity },
  );

  assert.equal(result.isRemoteWorkspace, true);
  assert.equal(result.remoteTarget, undefined);
});

test("LAN 目标透传 host/port 供主进程判定不支持", () => {
  const result = resolveWorkspaceOpenInEditorTarget(
    [
      workspaceTab({
        id: "t1",
        remoteTarget: { kind: "lan", host: "192.168.1.20", port: 45880, token: "secret" },
      }),
    ],
    { workspacePath: WORKSPACE_PATH },
  );

  assert.deepEqual(result.remoteTarget, { kind: "lan", host: "192.168.1.20", port: 45880 });
});

test("SSH 目标只保留连接标识，不把凭据带出 renderer", () => {
  const result = resolveWorkspaceOpenInEditorTarget(
    [
      workspaceTab({
        id: "t1",
        remoteTarget: {
          kind: "ssh",
          host: "10.0.0.8",
          port: 2222,
          username: "dev",
          sshConfigAlias: " dev-box ",
          password: "p@ss",
          privateKeyPassphrase: "secret",
        },
      }),
    ],
    { workspacePath: WORKSPACE_PATH },
  );

  assert.deepEqual(result.remoteTarget, {
    kind: "ssh",
    host: "10.0.0.8",
    port: 2222,
    username: "dev",
    sshConfigAlias: "dev-box",
  });
});

test("没有匹配 tab 或缺少工作区路径时不产出目标", () => {
  assert.deepEqual(resolveWorkspaceOpenInEditorTarget([], { workspacePath: WORKSPACE_PATH }), {
    isRemoteWorkspace: false,
    remoteTarget: undefined,
  });
  assert.deepEqual(resolveWorkspaceOpenInEditorTarget([], {}), {
    isRemoteWorkspace: false,
    remoteTarget: undefined,
  });
});

test("多个候选 tab 无法唯一定位时失败关闭，但仍标记为远端工作区", () => {
  const tabs: WindowTabState[] = [
    workspaceTab({ id: "t1", remoteTarget: v3Target }),
    workspaceTab({ id: "t2", remoteTarget: v3Target }),
  ];

  assert.deepEqual(resolveWorkspaceOpenInEditorTarget(tabs, { workspacePath: WORKSPACE_PATH }), {
    isRemoteWorkspace: true,
    remoteTarget: undefined,
  });
});
