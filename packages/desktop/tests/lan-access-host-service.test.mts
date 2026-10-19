/**
 * LR-1：`lan-access` Host 适配器的单元测试（假 credentials / settings / startServer）。
 *
 * 守的是适配器自己的语义，不是 `lanAccessServer` 的：
 * 1. 监听关闭时 `getState` 仍从凭据读配对记录；
 * 2. `ensureStarted` 是单一启动 Promise（并发 enable 只起一次监听）；
 * 3. 监听未开启时动作**明确失败**，不返回假成功；
 * 4. `removePairedPeer` / `renamePairedPeer` 真实写/删凭据（不再静默 no-op）；
 * 5. `onDidChange` 只在动作成功后发幂等快照；
 * 6. dispose 停监听；传入 `startLanAccessServer` 的集合不含 `lan-access` 频道。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  ICredentialService,
  ILanAccessService,
  ISettingService,
  ServiceCollection,
  type ILanAccessService as ILanAccessServiceShape,
} from "@mode/services";
import {
  LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  type LanAccessState,
} from "@mode/shared";

import { createLanAccessHostService } from "../src/host-standalone/lanAccessService.js";
import type {
  LanAccessServerHandle,
  StartLanAccessServerParams,
} from "../src/host/lanAccessServer.js";

const EMPTY_STATE: LanAccessState = {
  enabled: false,
  port: null,
  addresses: [],
  pairCode: null,
  clients: [],
};

function createFakeCredentials(): {
  credentials: ICredentialService;
  store: Map<string, string>;
} {
  const store = new Map<string, string>();
  const credentials = {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
  return { credentials, store };
}

function createFakeSettingService(
  initial: Record<string, unknown> = {},
): ISettingService & { current: () => Record<string, unknown> } {
  let settings: Record<string, unknown> = { ...initial };
  const service = {
    get: async () => settings,
    update: async (patch: Record<string, unknown>) => {
      settings = { ...settings, ...patch };
    },
    current: () => settings,
  };
  return service as unknown as ISettingService & { current: () => Record<string, unknown> };
}

function createFakeServerHandle(initial?: Partial<LanAccessState>): {
  handle: LanAccessServerHandle;
  disposed: () => number;
  state: () => LanAccessState;
} {
  let state: LanAccessState = { ...EMPTY_STATE, enabled: true, port: 45880, ...initial };
  let disposeCount = 0;
  const handle: LanAccessServerHandle = {
    getState: () => state,
    createPairCode: () => {
      state = { ...state, pairCode: { code: "ABCDEF", expiresAt: Date.now() + 60_000 } };
      return state;
    },
    removeClient: async (clientId: string) => {
      state = { ...state, clients: state.clients.filter((client) => client.id !== clientId) };
      return state;
    },
    resetTokens: async () => {
      state = { ...state, clients: [] };
      return state;
    },
    setClientRemoteDesktopAuthorized: async () => undefined,
    dispose: async () => {
      disposeCount += 1;
    },
  };
  return { handle, disposed: () => disposeCount, state: () => state };
}

function createServices(options: {
  credentials: ICredentialService;
  settingService: ISettingService;
}): ServiceCollection {
  const services = new ServiceCollection();
  services.register(ICredentialService, options.credentials);
  services.register(ISettingService, options.settingService);
  return services;
}

test("首个订阅者只拿最近一次快照，不重放历史", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    startServer: async () => createFakeServerHandle().handle,
  });
  try {
    await adapter.setEnabled({ enabled: true });
    const latest = await adapter.createPairCode();
    const received: LanAccessState[] = [];
    const subscription = adapter.onDidChange((state) => received.push(state));
    subscription.dispose();
    assert.equal(received.length, 1, "订阅时只补最近一条");
    assert.deepEqual(received[0], latest);
  } finally {
    await adapter.dispose();
  }
});

test("LAN 握手使用远程桌面 server 的能力位与 hooks", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const hooks = {
    requestSession: async () => ({ ok: true as const }),
    onSessionSocket: () => undefined,
    onMediaSocket: () => undefined,
  };
  let startParams: StartLanAccessServerParams | null = null;
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    getRemoteDesktop: () => ({ isServerEnabled: () => true, getLanHooks: () => hooks }),
    startServer: async (params) => {
      startParams = params;
      return createFakeServerHandle().handle;
    },
  });
  try {
    await adapter.setEnabled({ enabled: true });
    // 赋值发生在异步回调里，TS 的控制流分析看不到；这里显式恢复声明类型。
    const params = startParams as StartLanAccessServerParams | null;
    assert.ok(params);
    assert.equal(params.isRemoteDesktopServerEnabled?.(), true, "握手能力位取 RD server");
    assert.equal(params.remoteDesktop, hooks, "信令/媒体 socket 交给同一 owner");
  } finally {
    await adapter.dispose();
  }
});

test("监听关闭时 getState 仍从凭据读配对记录", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
  });
  try {
    // 内部生命周期方法不得进入 ProxyChannel 的 RPC 面。
    assert.equal(Object.keys(adapter).includes("dispose"), false);
    assert.equal(Object.keys(adapter).includes("getActiveServer"), false);
    // readDisabledLanAccessState 直接读凭据键；这里注入一条带 remoteDesktopAuthorized 的记录。
    await (credentials as unknown as { save(key: string, value: string): Promise<void> }).save(
      LAN_ACCESS_CLIENTS_CREDENTIAL_KEY,
      JSON.stringify([
        {
          id: "client-a",
          label: "Office-PC",
          createdAt: 1,
          lastUsedAt: null,
          remoteDesktopAuthorized: true,
        },
      ]),
    );
    const state = await adapter.getState();
    assert.equal(state.enabled, false);
    assert.equal(state.port, null);
    assert.deepEqual(state.addresses, []);
    assert.equal(state.pairCode, null);
    assert.equal(state.clients.length, 1);
    assert.equal(state.clients[0]?.remoteDesktopAuthorized, true);
  } finally {
    await adapter.dispose();
  }
});

test("并发 enable 只启动一次监听，并发出幂等状态快照", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const fake = createFakeServerHandle();
  let starts = 0;
  let passedServices: ServiceCollection | null = null;
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    startServer: async (params: StartLanAccessServerParams) => {
      starts += 1;
      passedServices = params.services;
      return fake.handle;
    },
  });
  const events: LanAccessState[] = [];
  const subscription = adapter.onDidChange((state) => events.push(state));
  try {
    const [first, second] = await Promise.all([
      adapter.setEnabled({ enabled: true }),
      adapter.setEnabled({ enabled: true }),
    ]);
    assert.equal(starts, 1, "并发 enable 必须共用同一次启动");
    assert.equal(first.enabled, true);
    assert.equal(second.enabled, true);
    // 首个订阅者先拿到 boot 快照，随后两个动作各发一次。
    assert.equal(events.length, 3, "订阅时补当前快照 + 每次动作一次");
    assert.equal(events[0]?.enabled, false, "订阅前是关闭态");
    assert.equal(events.at(-1)?.enabled, true);
    assert.equal(settingService.current().lanAccessEnabled, true, "开关必须持久化");

    // local-only：传给 LAN 服务端的集合里没有管理频道本身。
    assert.ok(passedServices);
    assert.equal(
      (passedServices as ServiceCollection).getOptional(ILanAccessService),
      undefined,
      "管理频道绝不能进入对端可见的服务集合",
    );
  } finally {
    subscription.dispose();
    await adapter.dispose();
  }
  assert.equal(fake.disposed(), 1, "dispose 必须停监听");
});

test("监听未开启时配对码/移除/重置明确失败", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    startServer: async () => createFakeServerHandle().handle,
  });
  try {
    await assert.rejects(adapter.createPairCode(), /未开启/u);
    await assert.rejects(adapter.removeClient({ clientId: "x" }), /未开启/u);
    await assert.rejects(adapter.resetTokens(), /未开启/u);
  } finally {
    await adapter.dispose();
  }
});

test("disable 会停监听并持久化开关", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const fake = createFakeServerHandle();
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    startServer: async () => fake.handle,
  });
  try {
    await adapter.setEnabled({ enabled: true });
    const state = await adapter.setEnabled({ enabled: false });
    assert.equal(state.enabled, false);
    assert.equal(fake.disposed(), 1);
    assert.equal(settingService.current().lanAccessEnabled, false);
  } finally {
    await adapter.dispose();
  }
});

test("启动失败时回写开关为 false 并抛出可读错误", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    startServer: async () => {
      throw new Error("EADDRINUSE");
    },
  });
  try {
    await assert.rejects(adapter.setEnabled({ enabled: true }), /EADDRINUSE/u);
    assert.equal(settingService.current().lanAccessEnabled, false, "失败不能留下 enabled=true");
    assert.equal((await adapter.getState()).enabled, false);
  } finally {
    await adapter.dispose();
  }
});

test("已配对对端：排序、重命名与删除都真实落凭据", async () => {
  const { credentials, store } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
  });
  try {
    store.set(buildLanPeerTokenKey("server-old"), "token-old");
    store.set(
      buildLanPeerMetaKey("server-old"),
      JSON.stringify({ host: "192.168.1.10", port: 45880, name: "Old", pairedAt: 10 }),
    );
    store.set(buildLanPeerTokenKey("server-new"), "token-new");
    store.set(
      buildLanPeerMetaKey("server-new"),
      JSON.stringify({ host: "192.168.1.11", port: 45880, name: "New", pairedAt: 20 }),
    );
    // 令牌在、元数据缺失：保留一行，至少还能删除。
    store.set(buildLanPeerTokenKey("server-bare"), "token-bare");

    const peers = await adapter.listPairedPeers();
    assert.deepEqual(
      peers.map((peer) => peer.serverId),
      ["server-new", "server-old", "server-bare"],
      "按 pairedAt 倒序；缺元数据的记录保留在最后",
    );
    assert.equal(peers[2]?.host, "");

    await adapter.renamePairedPeer({ serverId: "server-old", name: "  Renamed  " });
    const meta = JSON.parse(store.get(buildLanPeerMetaKey("server-old")) ?? "{}") as {
      name?: string;
      host?: string;
    };
    assert.equal(meta.name, "Renamed", "名称要 trim 后落盘");
    assert.equal(meta.host, "192.168.1.10", "重命名不动地址与令牌");

    await assert.rejects(adapter.renamePairedPeer({ serverId: "server-bare", name: "x" }));
    await assert.rejects(
      adapter.renamePairedPeer({ serverId: "server-old", name: "x".repeat(65) }),
      /1–64/u,
    );

    await adapter.removePairedPeer({ serverId: "server-old" });
    assert.equal(store.has(buildLanPeerTokenKey("server-old")), false);
    assert.equal(store.has(buildLanPeerMetaKey("server-old")), false);
    // 缺元数据也能删。
    await adapter.removePairedPeer({ serverId: "server-bare" });
    assert.equal(store.has(buildLanPeerTokenKey("server-bare")), false);
  } finally {
    await adapter.dispose();
  }
});

test("getPeerWorkspaces 在缺元数据/缺令牌时给出可读错误（不发网络请求）", async () => {
  const { credentials, store } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
  });
  try {
    await assert.rejects(adapter.getPeerWorkspaces({ serverId: "missing" }), /缺少地址信息/u);
    store.set(
      buildLanPeerMetaKey("no-token"),
      JSON.stringify({ host: "192.168.1.12", port: 45880 }),
    );
    await assert.rejects(adapter.getPeerWorkspaces({ serverId: "no-token" }), /尚未配对/u);
  } finally {
    await adapter.dispose();
  }
});

test("restoreOnStart 按设置恢复监听；关闭时不启动", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService({ lanAccessEnabled: true });
  const services = createServices({ credentials, settingService });
  let starts = 0;
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    startServer: async () => {
      starts += 1;
      return createFakeServerHandle().handle;
    },
  });
  try {
    assert.equal(starts, 1);
    assert.equal((await adapter.getState()).enabled, true);
  } finally {
    await adapter.dispose();
  }

  const secondSetting = createFakeSettingService({ lanAccessEnabled: false });
  const secondServices = createServices({
    credentials: createFakeCredentials().credentials,
    settingService: secondSetting,
  });
  let secondStarts = 0;
  const second = await createLanAccessHostService({
    services: secondServices,
    log: () => undefined,
    startServer: async () => {
      secondStarts += 1;
      return createFakeServerHandle().handle;
    },
  });
  await second.dispose();
  assert.equal(secondStarts, 0);
});

test("discoverPeers 委托给注入的发现实现", async () => {
  const { credentials } = createFakeCredentials();
  const settingService = createFakeSettingService();
  const services = createServices({ credentials, settingService });
  const adapter = await createLanAccessHostService({
    services,
    log: () => undefined,
    restoreOnStart: false,
    discover: async () => [
      {
        magic: "mode-lan",
        protocolVersion: 1,
        serverId: "peer-a",
        version: "1.0.0",
        port: 45880,
        requiresPairing: true,
        host: "192.168.1.20",
      },
    ],
  });
  try {
    const peers = await adapter.discoverPeers();
    assert.equal(peers[0]?.serverId, "peer-a");
  } finally {
    await adapter.dispose();
  }
  // 类型面：adapter 满足 ILanAccessService（编译期检查）。
  const typed: ILanAccessServiceShape = adapter;
  assert.ok(typed);
});
