/**
 * Host 窗口端口的 v4 可信连接作用域（spec: tauri-renderer-host §11.1）。
 *
 * 守的是「渲染层拿到的 IModeAgentService 必须已经带 trusted carrier」：
 * - 真实 `createModeAgentConnectionScope` + fake base：附件事务到达 base 时必须带
 *   `__modeTrustedV4Connection`，否则 base 会以 connectionUntrusted 拒绝；
 * - conversation share 复用同一个 scoped agent（share 内部持有 raw agent 会绕过作用域）；
 * - agent service 缺席时 overrides 为空，不伪造一个不可用频道。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  IConversationShareService,
  IModeAgentService,
  createModeAgentConnectionScope,
  type IConversationShareService as ConversationShareServiceShape,
  type IModeAgentService as ModeAgentServiceShape,
} from "@mode/services";
import { V4_WIRE_PROTOCOL_VERSION } from "@mode/shared/mode-protocol-v4";

import { createRendererConnectionScope } from "../src/host-standalone/rendererConnectionScope.js";

interface TrustedCarrier {
  connectionId?: string;
  clientMode?: string;
}

function readCarrier(params: unknown): TrustedCarrier | undefined {
  if (typeof params !== "object" || params === null) return undefined;
  return (params as { __modeTrustedV4Connection?: TrustedCarrier }).__modeTrustedV4Connection;
}

function createBaseAgent(onBegin?: (params: unknown) => void): ModeAgentServiceShape {
  return {
    async attachmentBeginV4(params: { uploadId: string }) {
      // 与真 modeAgentService 同一道门：没有 carrier 就拒绝，确保用例真的在守作用域。
      if (!readCarrier(params)) throw new Error("fault.attachment.connectionUntrusted");
      onBegin?.(params);
      return { uploadId: params.uploadId, state: "staging" as const, nextChunkIndex: 0 };
    },
    async setConnectionFlowStateV4() {
      return undefined;
    },
  } as unknown as ModeAgentServiceShape;
}

async function handshake(service: ModeAgentServiceShape): Promise<void> {
  await service.helloConversationV4();
  await service.initializeConversationV4({
    kind: "clientHello",
    protocolVersion: V4_WIRE_PROTOCOL_VERSION,
    clientId: "test-client",
    clientKind: "desktop",
    appVersion: "test",
  });
}

test("窗口端口作用域给附件事务注入可信连接载体", async () => {
  const seen: unknown[] = [];
  const base = createBaseAgent((params) => seen.push(params));
  const rendererScope = createRendererConnectionScope({
    agentService: base,
    agentChannelName: IModeAgentService.channelName,
    conversationShareChannelName: IConversationShareService.channelName,
    createConnectionScope: createModeAgentConnectionScope,
    conversationShareScopeFactory: Symbol("conversationShareConnectionScopeFactory"),
    connectionId: "host-rpc-test",
  });
  const scoped = rendererScope.overrides.get(
    IModeAgentService.channelName,
  ) as ModeAgentServiceShape;

  await handshake(scoped);
  await scoped.attachmentBeginV4({
    workspacePath: "D:/workspace",
    sessionId: "sess-1",
    uploadId: "upload-1",
    fileName: "shot.png",
    mime: "image/png",
    totalBytes: 4,
    totalChunks: 1,
    checksum: `sha256:${"0".repeat(64)}`,
  });

  assert.equal(seen.length, 1);
  const carrier = readCarrier(seen[0]);
  assert.equal(carrier?.connectionId, "host-rpc-test");
  assert.equal(carrier?.clientMode, "desktop-continuous");
  await rendererScope.dispose();
});

test("conversation share 复用同一个 scoped agent", async () => {
  const scopeFactory = Symbol("conversationShareConnectionScopeFactory");
  const base = createBaseAgent();
  const shareMarker = { kind: "scoped-share", agent: null as unknown };
  const shareService = {
    [scopeFactory](agent: ModeAgentServiceShape) {
      shareMarker.agent = agent;
      return shareMarker;
    },
  } as unknown as ConversationShareServiceShape;

  const rendererScope = createRendererConnectionScope({
    agentService: base,
    conversationShareService: shareService,
    agentChannelName: IModeAgentService.channelName,
    conversationShareChannelName: IConversationShareService.channelName,
    createConnectionScope: createModeAgentConnectionScope,
    conversationShareScopeFactory: scopeFactory,
    connectionId: "host-rpc-share",
  });

  const scopedAgent = rendererScope.overrides.get(IModeAgentService.channelName);
  assert.equal(rendererScope.overrides.get(IConversationShareService.channelName), shareMarker);
  assert.equal(shareMarker.agent, scopedAgent);
  await rendererScope.dispose();
});

test("agent service 缺席时不伪造 overrides", async () => {
  const rendererScope = createRendererConnectionScope({
    agentService: undefined,
    agentChannelName: IModeAgentService.channelName,
    conversationShareChannelName: IConversationShareService.channelName,
    createConnectionScope: createModeAgentConnectionScope,
    conversationShareScopeFactory: Symbol("unused"),
  });
  assert.equal(rendererScope.overrides.size, 0);
  await rendererScope.dispose();
});

test("Host dispose 会把 scope 一起释放", async () => {
  let disposed = 0;
  const fakeScopedAgent = createBaseAgent();
  const rendererScope = createRendererConnectionScope({
    agentService: createBaseAgent(),
    agentChannelName: IModeAgentService.channelName,
    conversationShareChannelName: IConversationShareService.channelName,
    createConnectionScope: () => ({
      service: fakeScopedAgent,
      setTransportFlowState: async () => undefined,
      dispose: async () => {
        disposed += 1;
      },
    }),
    conversationShareScopeFactory: Symbol("unused"),
    connectionId: "host-rpc-dispose",
  });

  assert.equal(rendererScope.overrides.get(IModeAgentService.channelName), fakeScopedAgent);
  await rendererScope.dispose();
  assert.equal(disposed, 1);
});
