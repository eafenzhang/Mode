import assert from "node:assert/strict";
import test from "node:test";

import { isSameRemoteTarget, type RemoteTarget } from "../src/remoteTarget.js";

// spec: docs/specs/im-bot-remote-workspace-binding.md「产品规则」——target 判等必须覆盖
// `RemoteTarget` 全部 kind，只比较定位对端所需的 authority；secret 与展示性字段不参与。
// 这条函数是 bot 连接状态查询、runtime attachment 与断线重连共用的唯一口径。

test("lan：同 host+port 判等，大小写与首尾空白归一", () => {
  const live: RemoteTarget = { kind: "lan", host: "192.168.1.20", port: 47821 };
  assert.equal(isSameRemoteTarget(live, { kind: "lan", host: "192.168.1.20", port: 47821 }), true);
  assert.equal(
    isSameRemoteTarget(live, { kind: "lan", host: " 192.168.1.20 ", port: 47821 }),
    true,
  );
  assert.equal(
    isSameRemoteTarget(
      { kind: "lan", host: "HOME-PC.local", port: 47821 },
      { kind: "lan", host: "home-pc.local", port: 47821 },
    ),
    true,
    "host 小写归一（DNS 名大小写不敏感）",
  );
  assert.equal(isSameRemoteTarget(live, { kind: "lan", host: "192.168.1.20", port: 47822 }), false);
  assert.equal(isSameRemoteTarget(live, { kind: "lan", host: "192.168.1.21", port: 47821 }), false);
});

test("lan：secret 与展示字段不影响判等", () => {
  const base: RemoteTarget = { kind: "lan", host: "10.0.0.5", port: 45880 };
  assert.equal(
    isSameRemoteTarget(base, {
      kind: "lan",
      host: "10.0.0.5",
      port: 45880,
      token: "one-time-token",
      serverName: "书房主机",
      serverId: "server-1",
      scheme: "http",
      path: "/d/tunnel",
    }),
    true,
    "token/serverName/serverId/path 不参与 identity",
  );
});

test("ssh：host 小写、port 缺省 22、username 与 privateKeyPath 参与；口令不参与", () => {
  const base: RemoteTarget = {
    kind: "ssh",
    host: "Build.Example.COM",
    port: 22,
    username: "dev",
    privateKeyPath: "/home/dev/.ssh/id_ed25519",
  };
  assert.equal(
    isSameRemoteTarget(base, {
      kind: "ssh",
      host: "build.example.com",
      username: "dev",
      privateKeyPath: "/home/dev/.ssh/id_ed25519",
      password: "secret",
    }),
    true,
    "缺省 port = 22；password 不参与",
  );
  assert.equal(
    isSameRemoteTarget(base, {
      kind: "ssh",
      host: "build.example.com",
      port: 2222,
      username: "dev",
    }),
    false,
    "port 不同判否",
  );
  assert.equal(
    isSameRemoteTarget(base, { kind: "ssh", host: "build.example.com", username: "root" }),
    false,
    "username 不同判否",
  );
  assert.equal(
    isSameRemoteTarget(base, {
      kind: "ssh",
      host: "build.example.com",
      username: "dev",
      privateKeyPath: "/tmp/other-key",
    }),
    false,
    "privateKeyPath 不同判否",
  );
  assert.equal(
    isSameRemoteTarget(base, {
      kind: "ssh",
      host: "build.example.com",
      username: "dev",
      privateKeyPath: "/home/dev/.ssh/id_ed25519",
      sshConfigAlias: "build",
    }),
    true,
    "sshConfigAlias 只是展示/选择入口，不参与判等",
  );
});

test("wsl：distro 缺省 default，user 参与", () => {
  const base: RemoteTarget = { kind: "wsl", distro: "Ubuntu", user: "dev" };
  assert.equal(isSameRemoteTarget(base, { kind: "wsl", distro: "Ubuntu", user: "dev" }), true);
  assert.equal(
    isSameRemoteTarget(base, { kind: "wsl", user: "dev" }),
    false,
    "Ubuntu 与缺省 default 不是同一个 distro",
  );
  assert.equal(
    isSameRemoteTarget({ kind: "wsl", user: "dev" }, { kind: "wsl", distro: "  ", user: "dev" }),
    true,
    "缺省 distro 与空白 distro 同义",
  );
  assert.equal(isSameRemoteTarget(base, { kind: "wsl", distro: "Ubuntu", user: "root" }), false);
});

test("docker：container 判等；kind 不同一律判否", () => {
  assert.equal(
    isSameRemoteTarget(
      { kind: "docker", container: "dev-box" },
      { kind: "docker", container: "dev-box" },
    ),
    true,
  );
  assert.equal(
    isSameRemoteTarget(
      { kind: "docker", container: "dev-box" },
      { kind: "docker", container: "other" },
    ),
    false,
  );
  assert.equal(
    isSameRemoteTarget(
      { kind: "lan", host: "dev-box", port: 22 },
      { kind: "docker", container: "dev-box" },
    ),
    false,
    "kind 不同必须判否（同名 host/container 也不例外）",
  );
  assert.equal(
    isSameRemoteTarget(
      { kind: "ssh", host: "dev-box", username: "dev" },
      { kind: "wsl", distro: "dev-box" },
    ),
    false,
  );
});
