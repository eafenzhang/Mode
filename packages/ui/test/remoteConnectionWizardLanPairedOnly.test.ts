import assert from "node:assert/strict";
import test from "node:test";

import type { LanPairedPeer } from "@mode/shared";
import { buildAvailableKinds } from "../src/hooks/useRemoteConnectionForm.js";
import { buildRemoteTarget } from "../src/lib/remoteConnectionWizard.js";

/**
 * 向导「局域网」：只允许选择已配对设备（docs/specs/lan-paired-devices.md §向导）。
 * 本文件只覆盖纯函数，避免为了断言把整个连接向导渲染起来。
 */

const intl = {
  formatMessage: ({ id }: { id: string }) => id,
};

const baseSnapshot = {
  kind: "lan" as const,
  host: "",
  port: "",
  username: "",
  sshAuthMethod: "password" as const,
  password: "",
  privateKeyPath: "",
  privateKeyPassphrase: "",
  wslDistro: "",
  dockerContainer: "",
  lanPeer: null as LanPairedPeer | null,
};

test("连接方式包含局域网，且排在 SSH/WSL/Docker 之后", () => {
  assert.deepEqual(buildAvailableKinds({ isWindowsDesktop: true }), [
    "ssh",
    "wsl",
    "docker",
    "lan",
  ]);
  assert.deepEqual(buildAvailableKinds({ isWindowsDesktop: false }), ["ssh", "docker", "lan"]);
});

test("未选择已配对设备时给出选择校验，不回落到手动地址", () => {
  const result = buildRemoteTarget(intl, baseSnapshot);
  assert.equal(result.target, undefined);
  assert.equal(result.errorMessage, "remote.lan.validation.pairedRequired");
});

test("提交已配对设备时保留完整 endpoints（公网优先）", () => {
  const peer: LanPairedPeer = {
    serverId: "server-1",
    host: "relay.example.com",
    port: 443,
    name: "书房电脑",
    endpoints: [
      { scheme: "http", host: "192.168.1.20", port: 45880, kind: "lan" },
      { scheme: "https", host: "relay.example.com", port: 443, path: "/d/t1", kind: "public" },
    ],
  };

  const result = buildRemoteTarget(intl, { ...baseSnapshot, lanPeer: peer });

  assert.deepEqual(result.target, {
    kind: "lan",
    host: "relay.example.com",
    port: 443,
    scheme: "https",
    path: "/d/t1",
    endpoints: [
      { scheme: "https", host: "relay.example.com", port: 443, path: "/d/t1", kind: "public" },
      { scheme: "http", host: "192.168.1.20", port: 45880, kind: "lan" },
    ],
    serverId: "server-1",
    serverName: "书房电脑",
  });
});

test("旧配对记录缺少地址时提示重新配对", () => {
  const result = buildRemoteTarget(intl, {
    ...baseSnapshot,
    lanPeer: { serverId: "legacy", host: "", port: 0 },
  });
  assert.equal(result.target, undefined);
  assert.equal(result.errorMessage, "remote.lan.validation.pairedUnavailable");
});
