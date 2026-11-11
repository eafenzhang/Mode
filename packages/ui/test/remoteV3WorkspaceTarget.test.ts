import assert from "node:assert/strict";
import test from "node:test";

import {
  buildRemoteEnvironmentKey,
  buildRemoteWorkspaceIdentity,
  isSameRemoteTarget,
  type RemoteV3DeviceSummary,
} from "@mode/shared";

import { buildRemoteTarget } from "../src/lib/remoteConnectionWizard.js";
import {
  createRemoteTargetFromSnapshot,
  createRemoteTargetSnapshot,
  formatRemoteWorkspaceHeaderHostLabel,
  formatRemoteWorkspaceTargetSubtitle,
} from "../src/lib/remoteWorkspaceHistory.js";

const HOST: RemoteV3DeviceSummary = {
  deviceId: "019a0000-0000-7000-8000-000000000099",
  name: "书房主机",
  role: "host",
  platform: "desktop",
  identityKey: "identity",
  online: true,
  addresses: [],
};

const intl = {
  formatMessage: ({ id }: { id: string }) => id,
};

function wizardSnapshot(v3Host: RemoteV3DeviceSummary | null) {
  return {
    kind: "v3" as const,
    host: "",
    port: "",
    username: "",
    sshAuthMethod: "password" as const,
    password: "",
    privateKeyPath: "",
    privateKeyPassphrase: "",
    wslDistro: "",
    dockerContainer: "",
    v3Host,
  };
}

test("v3 目标身份只取 deviceId：展示名不参与判等", () => {
  const left = { kind: "v3" as const, deviceId: HOST.deviceId, deviceName: "旧名字" };
  const right = { kind: "v3" as const, deviceId: HOST.deviceId, deviceName: "新名字" };
  assert.equal(isSameRemoteTarget(left, right), true);
  assert.equal(
    buildRemoteWorkspaceIdentity("/home/me/project", left),
    `remote:v3:${HOST.deviceId}:/home/me/project`,
  );
  assert.equal(buildRemoteEnvironmentKey(left), `v3:${HOST.deviceId}`);
});

test("v3 历史快照往返只保留 deviceId 与展示名", () => {
  const target = { kind: "v3" as const, deviceId: HOST.deviceId, deviceName: HOST.name };
  const snapshot = createRemoteTargetSnapshot("key", target);
  assert.deepEqual(snapshot, {
    kind: "v3",
    deviceId: HOST.deviceId,
    deviceName: HOST.name,
  });
  const restored = createRemoteTargetFromSnapshot(snapshot, {
    password: null,
    privateKeyPassphrase: null,
  });
  assert.deepEqual(restored, target);
  assert.equal(formatRemoteWorkspaceTargetSubtitle(snapshot), `v3 · ${HOST.name}`);
  assert.equal(formatRemoteWorkspaceHeaderHostLabel(snapshot), HOST.name);
});

test("v3 向导只接受在线 Host：未选、离线、在线三种结果", () => {
  assert.deepEqual(buildRemoteTarget(intl, wizardSnapshot(null)), {
    errorMessage: "remote.v3.validation.selectHost",
  });
  assert.deepEqual(buildRemoteTarget(intl, wizardSnapshot({ ...HOST, online: false })), {
    errorMessage: "remote.v3.validation.hostOffline",
  });
  assert.deepEqual(buildRemoteTarget(intl, wizardSnapshot(HOST)), {
    target: {
      kind: "v3",
      deviceId: HOST.deviceId,
      deviceName: HOST.name,
    },
  });
});
