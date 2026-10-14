/**
 * 对话记录镜像服务的存储回归（docs/specs/remote-workspace-conversation-mirror.md）。
 * 运行：npx tsx --test packages/services/test/conversationMirrorService.test.ts
 *
 * 钉住三件事：
 * 1) 同步域是 workspacePath：同一路径无论以哪种形态书写都落同一个目录，不同路径互不串；
 * 2) 一个 origin 一份文件、覆盖式幂等；损坏文件只跳过自己；
 * 3) 拒绝自己 origin 的发布（防回声），非法 payload 不落盘。
 */
import assert from "node:assert/strict";
import test from "node:test";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 数据目录隔离必须在 @mode/services 首次 import 前生效（paths 在模块加载时读取环境变量）。
const dataDir = mkdtempSync(join(tmpdir(), "mode-conversation-mirror-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
process.env.MODE_DESKTOP_HOME_DIR = dataDir;

test.after(() => {
  rmSync(dataDir, { recursive: true, force: true });
});

const WORKSPACE_A = "D:/proj";
const WORKSPACE_B = "D:/other";

function snapshot(params: {
  originDeviceId: string;
  workspacePath: string;
  sessionId: string;
  text: string;
  updatedAt?: number;
}) {
  return {
    version: 1 as const,
    originDeviceId: params.originDeviceId,
    originLabel: `label-${params.originDeviceId}`,
    generatedAt: 1_000,
    workspacePath: params.workspacePath,
    records: [
      {
        sessionId: params.sessionId,
        title: `title-${params.sessionId}`,
        createdAt: 1,
        updatedAt: params.updatedAt ?? 2,
        messageCount: 1,
        truncated: false,
        messages: [{ role: "user" as const, text: params.text }],
      },
    ],
  };
}

test("store：按工作区路径隔离，覆盖式写入同一 origin", async () => {
  const { createConversationMirrorStore } = await import(
    "../src/conversation-mirror/conversationMirrorStore.js"
  );
  const store = createConversationMirrorStore();

  await store.writeSnapshot(snapshot({ originDeviceId: "dev-peer", workspacePath: WORKSPACE_A, sessionId: "s1", text: "第一版" }));
  await store.writeSnapshot(snapshot({ originDeviceId: "dev-peer", workspacePath: WORKSPACE_A, sessionId: "s1", text: "第二版" }));
  await store.writeSnapshot(snapshot({ originDeviceId: "dev-other", workspacePath: WORKSPACE_A, sessionId: "s2", text: "别人的" }));

  const forA = await store.readWorkspaceSnapshots(WORKSPACE_A);
  assert.equal(forA.length, 2);
  const peer = forA.find((item) => item.originDeviceId === "dev-peer");
  assert.equal(peer?.records[0]?.messages[0]?.text, "第二版");

  const forB = await store.readWorkspaceSnapshots(WORKSPACE_B);
  assert.deepEqual(forB, []);
});

test("store：路径形态归一（分隔符/收尾斜杠）后落同一目录", async () => {
  const { getConversationMirrorDir } = await import("../src/paths.js");
  const { createConversationMirrorStore } = await import(
    "../src/conversation-mirror/conversationMirrorStore.js"
  );
  const store = createConversationMirrorStore();

  await store.writeSnapshot(
    snapshot({
      originDeviceId: "dev-peer",
      workspacePath: "D:\\mirror-test\\gamma\\",
      sessionId: "s1",
      text: "归一",
    }),
  );

  const readBack = await store.readWorkspaceSnapshots("D:/mirror-test/gamma");
  assert.equal(readBack.length, 1);
  assert.equal(
    getConversationMirrorDir("D:\\mirror-test\\gamma\\"),
    getConversationMirrorDir("D:/mirror-test/gamma"),
  );
  assert.notEqual(
    getConversationMirrorDir("D:/mirror-test/gamma"),
    getConversationMirrorDir("D:/mirror-test/delta"),
  );
});

test("store：损坏文件只跳过自己，目录不存在返回空", async () => {
  const { getConversationMirrorDir } = await import("../src/paths.js");
  const { createConversationMirrorStore } = await import(
    "../src/conversation-mirror/conversationMirrorStore.js"
  );
  const store = createConversationMirrorStore();

  await store.writeSnapshot(
    snapshot({ originDeviceId: "dev-good", workspacePath: WORKSPACE_A, sessionId: "s1", text: "好的" }),
  );
  const dir = getConversationMirrorDir(WORKSPACE_A);
  writeFileSync(join(dir, "broken.json"), "{ this is not json");
  writeFileSync(join(dir, "unknown-version.json"), JSON.stringify({ version: 99 }));

  const snapshots = await store.readWorkspaceSnapshots(WORKSPACE_A);
  assert.deepEqual(
    snapshots.map((item) => item.originDeviceId),
    ["dev-good"],
  );
  assert.deepEqual(await store.readWorkspaceSnapshots("D:/never-written"), []);
});

test("service：对端通道拒绝本机 origin（回声），自快照通道接受本机、拒绝外来 origin", async () => {
  const { createConversationMirrorService } = await import(
    "../src/conversation-mirror/conversationMirrorService.js"
  );
  const service = createConversationMirrorService();
  const identity = await service.getMirrorIdentity();
  assert.ok(identity.deviceId.trim().length > 0);
  assert.ok(identity.label.trim().length > 0);

  // 对端发布通道收到本机自己的快照 = 回声，不得落盘。
  const selfPublished = await service.publishSnapshot({
    snapshot: snapshot({ originDeviceId: identity.deviceId, workspacePath: WORKSPACE_B, sessionId: "s1", text: "自己" }),
  });
  assert.deepEqual(selfPublished, { stored: false, reason: "self-origin", recordCount: 0 });

  // 本机自快照通道：本机 origin 落盘（供对端经 RPC 读取）。
  const localWritten = await service.writeLocalSnapshot({
    snapshot: snapshot({ originDeviceId: identity.deviceId, workspacePath: "D:/self", sessionId: "self-1", text: "本机" }),
  });
  assert.equal(localWritten.stored, true);

  // 本机自快照通道不得被对端冒充。
  const forged = await service.writeLocalSnapshot({
    snapshot: snapshot({ originDeviceId: "dev-attacker", workspacePath: "D:/self", sessionId: "x", text: "冒充" }),
  });
  assert.deepEqual(forged, { stored: false, reason: "foreign-origin", recordCount: 0 });
  assert.deepEqual(await service.readWorkspaceSnapshots({ workspacePath: "D:/self" }), {
    snapshots: [
      snapshot({ originDeviceId: identity.deviceId, workspacePath: "D:/self", sessionId: "self-1", text: "本机" }),
    ],
  });

  const peerPublished = await service.publishSnapshot({
    snapshot: snapshot({ originDeviceId: "dev-peer", workspacePath: WORKSPACE_B, sessionId: "s1", text: "对端" }),
  });
  assert.equal(peerPublished.stored, true);
  assert.equal(peerPublished.recordCount, 1);

  const invalid = await service.publishSnapshot({
    snapshot: { version: 1, originDeviceId: "dev-peer" } as never,
  });
  assert.equal(invalid.stored, false);
  assert.equal(invalid.reason, "invalid-record");

  const { snapshots } = await service.readWorkspaceSnapshots({ workspacePath: WORKSPACE_B });
  assert.deepEqual(
    snapshots.map((item) => item.originDeviceId),
    ["dev-peer"],
  );
});

test("service：发布超限快照被截断后落盘（字节/条数上限不落原样）", async () => {
  const { createConversationMirrorService } = await import(
    "../src/conversation-mirror/conversationMirrorService.js"
  );
  const { CONVERSATION_MIRROR_LIMITS } = await import("@mode/shared");
  const service = createConversationMirrorService();
  const oversized = {
    version: 1 as const,
    originDeviceId: "dev-huge",
    originLabel: "huge",
    generatedAt: 1,
    workspacePath: "D:/huge",
    records: Array.from({ length: CONVERSATION_MIRROR_LIMITS.maxRecords + 10 }, (_unused, index) => ({
      sessionId: `s${index}`,
      title: `t${index}`,
      createdAt: index,
      updatedAt: index,
      messageCount: 1,
      truncated: false,
      messages: [{ role: "user" as const, text: "x" }],
    })),
  };

  const result = await service.publishSnapshot({ snapshot: oversized });
  assert.equal(result.recordCount, CONVERSATION_MIRROR_LIMITS.maxRecords);

  const { snapshots } = await service.readWorkspaceSnapshots({ workspacePath: "D:/huge" });
  assert.equal(snapshots[0]?.records.length, CONVERSATION_MIRROR_LIMITS.maxRecords);
});

test("store：一个 origin 只产生一个文件", async () => {
  const { getConversationMirrorDir } = await import("../src/paths.js");
  const { createConversationMirrorStore } = await import(
    "../src/conversation-mirror/conversationMirrorStore.js"
  );
  const store = createConversationMirrorStore();

  const workspacePath = "D:/mirror-test/dup";
  await store.writeSnapshot(snapshot({ originDeviceId: "dev-dup", workspacePath, sessionId: "s1", text: "一" }));
  await store.writeSnapshot(
    snapshot({ originDeviceId: "dev-dup", workspacePath, sessionId: "s1", text: "二", updatedAt: 9 }),
  );

  // 一个 origin 一个文件：重复发布是覆盖，不是追加。
  assert.deepEqual(readdirSync(getConversationMirrorDir(workspacePath)), ["dev-dup.json"]);

  const snapshots = await store.readWorkspaceSnapshots(workspacePath);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0]?.records[0]?.updatedAt, 9);
});
