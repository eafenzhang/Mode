import assert from "node:assert/strict";
import test from "node:test";
import {
  buildRemoteEnvironmentKey,
  buildRemoteWorkspaceIdentity,
  createOpenInEditorRemoteTarget,
  parseRemoteWorkspaceIdentity,
  remoteTargetSchema,
  stripRemoteTargetSecrets,
  type LanConnectOptions,
} from "../src/index.js";

const lanTarget: LanConnectOptions = {
  kind: "lan",
  host: "192.168.1.20",
  port: 45880,
  serverId: "server-abc",
  serverName: "书房电脑",
  token: "secret-token",
};

test("remoteTargetSchema 接受 lan 目标并校验端口范围", () => {
  const parsed = remoteTargetSchema.parse(lanTarget);
  assert.equal(parsed.kind, "lan");
  assert.equal(parsed.host, "192.168.1.20");
  assert.equal(parsed.port, 45880);

  assert.equal(
    remoteTargetSchema.safeParse({ ...lanTarget, port: 0 }).success,
    false,
    "端口 0 非法",
  );
  assert.equal(
    remoteTargetSchema.safeParse({ ...lanTarget, port: 70000 }).success,
    false,
    "端口越界非法",
  );
  assert.equal(
    remoteTargetSchema.safeParse({ ...lanTarget, host: "  " }).success,
    false,
    "空主机名非法",
  );
});

test("stripRemoteTargetSecrets 只去掉 lan 的 token，保留其余字段", () => {
  const sanitized = stripRemoteTargetSecrets(lanTarget);
  assert.equal(sanitized.kind, "lan");
  assert.equal("token" in sanitized, false, "token 不得进入长期状态/回包");
  assert.equal((sanitized as LanConnectOptions).serverId, "server-abc");
  assert.equal((sanitized as LanConnectOptions).port, 45880);
});

test("lan workspace identity 往返：host 小写、路径归一、address 段完整", () => {
  const identity = buildRemoteWorkspaceIdentity("D:\\Work\\Demo\\", {
    ...lanTarget,
    host: "192.168.1.20",
  } as LanConnectOptions);
  assert.equal(identity, "remote:lan:192.168.1.20:45880:/D:/Work/Demo");

  const parsed = parseRemoteWorkspaceIdentity(identity);
  assert.deepEqual(parsed, { kind: "lan", workspacePath: "/D:/Work/Demo" });
});

test("parseRemoteWorkspaceIdentity 拒绝缺段或路径不以 / 开头的 lan identity", () => {
  assert.equal(parseRemoteWorkspaceIdentity("remote:lan:192.168.1.20:/x"), null, "缺 port 段");
  assert.equal(parseRemoteWorkspaceIdentity("remote:lan:192.168.1.20:45880"), null, "缺路径段");
  assert.equal(parseRemoteWorkspaceIdentity("remote:lan:192.168.1.20:45880:x"), null, "路径必须绝对");
});

test("buildRemoteEnvironmentKey 为 lan 生成稳定环境键", () => {
  assert.equal(buildRemoteEnvironmentKey(lanTarget), "lan:192.168.1.20:45880");
  assert.equal(
    buildRemoteEnvironmentKey({ ...lanTarget, host: " Study-PC " }),
    "lan:study-pc:45880",
    "host 大小写/空白归一",
  );
});

test("createOpenInEditorRemoteTarget 透出 lan 地址供主进程判定不支持", () => {
  assert.deepEqual(createOpenInEditorRemoteTarget(lanTarget), {
    kind: "lan",
    host: "192.168.1.20",
    port: 45880,
  });
});

test("lan 发现报文：编码可被解析，坏报文一律丢弃", async () => {
  const { buildLanAnnouncementPayload, buildLanProbePayload, parseLanAnnouncement } = await import(
    "../src/lanAccess.js"
  );
  const payload = buildLanAnnouncementPayload({
    serverId: "server-1",
    name: "书房电脑",
    version: "1.2.3",
    port: 45880,
    requiresPairing: true,
    platform: "win32",
  });
  const parsed = parseLanAnnouncement(payload);
  assert.equal(parsed?.serverId, "server-1");
  assert.equal(parsed?.port, 45880);
  assert.equal(parsed?.requiresPairing, true);

  // 探测报文本身不是应答，不能被当成设备。
  assert.equal(parseLanAnnouncement(buildLanProbePayload()), null);
  assert.equal(parseLanAnnouncement("not-json"), null);
  assert.equal(parseLanAnnouncement(JSON.stringify({ magic: "mode-lan" })), null, "缺字段丢弃");
  assert.equal(
    parseLanAnnouncement(JSON.stringify({ ...JSON.parse(payload), port: 0 })),
    null,
    "非法端口丢弃",
  );
  assert.equal(
    parseLanAnnouncement(JSON.stringify({ ...JSON.parse(payload), protocolVersion: 999 })),
    null,
    "协议版本不匹配丢弃",
  );
});

test("lan 访问状态 schema 与配对码格式", async () => {
  const { isLanAccessPairCodeFormat, lanAccessStateSchema } = await import("../src/lanAccess.js");
  const state = {
    enabled: true,
    port: 45880,
    addresses: ["192.168.1.20:45880"],
    pairCode: { code: "A1B2C3", expiresAt: Date.now() + 60_000 },
    clients: [{ id: "c1", label: "Windows", createdAt: 1, lastUsedAt: null }],
  };
  assert.equal(lanAccessStateSchema.safeParse(state).success, true);
  assert.equal(
    lanAccessStateSchema.safeParse({ ...state, port: null, pairCode: null, enabled: false }).success,
    true,
    "关闭态：port/pairCode 为 null 合法",
  );
  assert.equal(lanAccessStateSchema.safeParse({ ...state, clients: [{ id: "" }] }).success, false);

  assert.equal(isLanAccessPairCodeFormat("a1b2c3"), true);
  assert.equal(isLanAccessPairCodeFormat("A1B2C3"), true);
  assert.equal(isLanAccessPairCodeFormat("A1B2C"), false, "长度不足");
  assert.equal(isLanAccessPairCodeFormat("A1B2C3D"), false, "长度超限");
  assert.equal(isLanAccessPairCodeFormat("ZZZZZZ"), false, "非十六进制");
});
