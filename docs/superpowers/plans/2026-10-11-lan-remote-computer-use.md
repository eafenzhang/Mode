# LAN 远端工作区电脑控制（完整闭环）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 局域网（LAN attach）远端工作区的对话获得完整电脑控制闭环：输入框发起、对话内工具卡片/截图展示、对话内权限授权与对端 OS 权限引导。

**Architecture:** 对端 LAN access server 在 `/api/server-info` 声明设备能力位 `capabilities.computerUse`，经 host descriptor 消息（`remote-workspace-connected`）下发 main → renderer attachment 参数 → `RemoteWorkspaceSession.capabilities`；UI 四门据此与对端插件启用态判定支持。对端 host（desktop-local authority）对 `remote:lan:` identity 放行 CUA helper admission；权限服务经远端服务集合透传，权限观察事件照 telemetry relay 先例放行，`ICuaPermissionService.openPermissionOnboarding` 经 host→main 消息打开**对端机器**的引导窗口。

**Tech Stack:** TypeScript、Electron（main/host utility/preload/renderer 四进程）、zod schema、zustand store、node:test + tsx。

**Spec:** `docs/specs/lan-remote-computer-use.md`（含 2026-10-11 修订）、`docs/specs/computer-use-enablement.md`、`docs/specs/lan-remote-attach.md`。计划论证全部来自 spec；执行者须同时阅读 spec。

**Plan 特有说明（读源码的时点）：** 本计划的关键行号锚点在探索时核对过；执行每个任务前先读「Files」列出的目标区段（±30 行），以当前源码为准落笔，行号漂移不阻塞。

## Global Constraints

- 开工先跑 `node scripts/check-workspace-freshness.mjs` 确认基线；Node 版本以 `mise.toml` 为准。
- 代码改动先跑 `pnpm architecture:check --changed`，并用 `pnpm architecture:context <module-id>` 读目标模块受控上下文（架构治理技能 `.agents/skills/architecture-governance/SKILL.md`）。
- clean-room：只读本仓库源码；不查阅上游 zai-org/ZCode 及其衍生实现。
- git：每个 Task 至少一次本地提交；**只 `git add` 本计划列出的文件**（工作区有大量与本任务无关的本地改动，禁止 `git add -A` / `git add .`）；**禁止任何 `git push`**。
- 行为改动处写中文注释，说明原因与依据（指向 spec 文件名）。
- 验证全套：`pnpm typecheck`、`pnpm lint`、`pnpm fmt:check`（不通过先 `pnpm fmt:fix`）、`pnpm architecture:check --changed`、`node scripts/check-language-policy.mjs`。测试按包入口跑：`pnpm --filter @mode/shared test`、`--filter @mode/services test`、`--filter @mode/server test`、desktop `node --test "packages/desktop/tests/*.test.mjs"` + `node --test "packages/desktop/tests/*.test.mts"`、ui 手动 `pnpm exec tsx --test packages/ui/test/*.test.ts`。
- 不做 ssh/wsl/docker、手机远控（`web-remote-replayable`）、IM Bot 的任何放行；这些形态必须维持现有行为（回归断言写进对应测试）。
- `packages/ui` 无 test 脚本、不进 CI：新增 ui 测试必须在报告中注明「手动执行」。
- i18n 新增键必须同时写 `zh-CN.ts`、`en-US.ts`、`fa.ts`（fa 缺键会显示原始 key）。

## 文件结构总览

| 任务 | 文件 | 职责 |
|---|---|---|
| T1 | `packages/shared/src/server-remote.ts`、`packages/server/src/http.ts`、`packages/desktop/src/host/lanAccessServer.ts` | 握手能力位 schema 与对端填充 |
| T2 | `packages/shared/src/validation.ts`、`packages/desktop/src/host/index.ts`、`packages/desktop/src/host/windowRemoteConnectionRegistry.ts`（只读依赖）、`packages/desktop/src/main/desktopRemoteSessions.ts`、`packages/desktop/src/preload/index.ts`、`packages/desktop/src/renderer/src/remoteWorkspaceServicePortBridge.ts`、`packages/desktop/src/renderer/src/main.tsx`、`packages/ui/src/store/remoteWorkspaceSessionStore.ts` | 能力位经 descriptor→attachment 参数→store 下发 renderer |
| T3 | `packages/services/src/cuaEnablement.ts`、`packages/services/src/node.ts` | helper admission 对 lan identity 放行 |
| T4 | `packages/ui/src/settings/computerUseAvailability.ts`、`packages/ui/src/hooks/useRemoteWorkspaceComputerUseCapability.ts`（新）、`packages/ui/src/hooks/useCuaComposerEntry.ts`、`packages/ui/src/settings/ComputerUseSection.tsx`、`packages/ui/src/settings/PluginsSection.tsx` | UI 四门改判 |
| T5 | `packages/desktop/src/host/remoteWorkspaceServiceCollection.ts`、`packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts` | 权限服务透传与 remote pane 归属 |
| T6 | `packages/services/src/mode-agent/modeAgentService.ts`、`packages/services/src/mode-agent/modeAgentConnectionScope.ts`、`packages/services/src/mode-agent/modeAgent.ts` | 权限观察事件面放行 |
| T7 | `packages/services/src/cua-permission-broker/cuaPermissionService.ts`、`packages/services/src/node.ts`、`packages/shared/src/channels.ts`、`packages/shared/src/validation.ts`、`packages/desktop/src/host/index.ts`、`packages/desktop/src/main/desktopHostProcess.ts` | `openPermissionOnboarding` host→main 管道 |
| T8 | `packages/ui/src/cua-permission/CuaPermissionObservationAttachment.tsx`、`packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts` | 多源订阅、对端路由、文案 |
| T9 | `docs/specs/lan-remote-computer-use.md` | 验收勾选与注释同步 |

---

### Task 1: 握手能力位 schema 与对端填充

**Files:**
- Modify: `packages/shared/src/server-remote.ts:18-23`（capabilities 对象）
- Modify: `packages/server/src/http.ts:224-248`（`HttpServerOptions`）、`:281-296`（`createServerInfo`）
- Modify: `packages/desktop/src/host/lanAccessServer.ts:232-243`（`createHttpServer` 调用）
- Test: `packages/shared/test/server-remote-capabilities.test.ts`（新）、`packages/server/test/lan-server-info-computer-use.test.ts`（新）

**Interfaces:**
- Consumes: `isModeCuaInternalFeatureEnabled`（`packages/shared/src/runtimeEnv.ts:34` 导出名，先 `grep -n "export.*isModeCuaInternalFeatureEnabled" packages/shared/src/runtimeEnv.ts` 确认；若非导出，从同文件按实际名导入）。
- Produces: `serverRemoteInfoSchema.capabilities.computerUse?: boolean`（旧对端缺键解析通过）；`HttpServerOptions.computerUseCapability?: boolean`；LAN access server 恒发布尔值。

- [ ] **Step 1: 写共享 schema 失败测试**

新建 `packages/shared/test/server-remote-capabilities.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { serverRemoteInfoSchema } from "../src/server-remote.js";

// server-info capabilities 的 computerUse 位：旧对端缺键必须解析通过（握手降级为不支持），
// 新对端必须是布尔；非布尔 fail closed。依据 docs/specs/lan-remote-computer-use.md「旧对端降级」。
const baseInfo = {
  serverId: "srv-1",
  version: "1.0.0",
  protocolVersion: 1 as const,
  authRequired: false,
  workspaces: [],
  capabilities: { desktopContinuous: true as const, websocketRpc: true as const },
};

test("旧对端 capabilities 无 computerUse 键仍解析通过", () => {
  const parsed = serverRemoteInfoSchema.safeParse(baseInfo);
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.computerUse, undefined);
});

test("新对端 computerUse 布尔位保留", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, computerUse: true },
  });
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.computerUse, true);
});

test("computerUse 非布尔解析失败（fail closed）", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, computerUse: "yes" },
  });
  assert.equal(parsed.success, false);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/shared test`
Expected: FAIL（`computerUse` 不是已知键——zod 默认 strip 不报错则断言 2/3 失败；若完全绿说明 schema 行为与预期不符，回到 Step 3 后补 `z.strictObject` 语义确认，**不得**把失败改成跳过）。

- [ ] **Step 3: 实现 schema**

`packages/shared/src/server-remote.ts` capabilities 对象内追加：

```ts
  capabilities: z.object({
    desktopContinuous: z.literal(true),
    websocketRpc: z.literal(true),
    // 旧 Server 缺少新增 dynamic event，必须先声明能力再订阅，避免异常打进对端读循环。
    // LAN 电脑控制设备能力位：对端 win/mac + env kill-switch + desktop-local host 才发 true；
    // 旧对端缺键 = 不支持（docs/specs/lan-remote-computer-use.md）。
    computerUse: z.boolean().optional(),
  }),
```

- [ ] **Step 4: 写 server 端点失败测试**

新建 `packages/server/test/lan-server-info-computer-use.test.ts`（harness 对齐 `lan-cookie-compat.test.ts`：最小 services 桩 + 真实 `createHttpServer`）：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import type { ServiceCollection } from "@mode/services";
import { serverRemoteInfoSchema } from "@mode/shared";
import { createHttpServer } from "../src/http.js";

async function fetchServerInfo(options: { computerUseCapability?: boolean }) {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    ...(options.computerUseCapability !== undefined
      ? { computerUseCapability: options.computerUseCapability }
      : {}),
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/server-info`);
    assert.equal(response.status, 200);
    const parsed = serverRemoteInfoSchema.safeParse(await response.json());
    assert.equal(parsed.success, true, "server-info 必须通过共享 schema");
    return parsed.success ? parsed.data : undefined;
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

test("声明 computerUseCapability:true 时 capabilities.computerUse 为 true", async () => {
  const info = await fetchServerInfo({ computerUseCapability: true });
  assert.equal(info?.capabilities.computerUse, true);
});

test("声明 computerUseCapability:false 时键存在且为 false", async () => {
  const info = await fetchServerInfo({ computerUseCapability: false });
  assert.equal(info?.capabilities.computerUse, false);
});

test("未声明时键缺省（其余消费方与旧对端行为不变）", async () => {
  const info = await fetchServerInfo({});
  assert.equal(info?.capabilities.computerUse, undefined);
});
```

- [ ] **Step 5: 跑测试确认失败**

Run: `pnpm --filter @mode/server test`
Expected: FAIL（未知 options 字段被忽略 → `computerUse` undefined → 断言 1/2 失败；类型层也应报 `computerUseCapability` 不在 `HttpServerOptions`）。

- [ ] **Step 6: 实现 options 与填充**

`packages/server/src/http.ts`：

1. `HttpServerOptions`（:224-248 一带）追加字段：

```ts
  /**
   * 设备级电脑控制能力（LAN 能力协商）。只有具备 helper/host 条件的 server
   * （desktop LAN access server）会传布尔值；不传 = 不声明该键（docs/specs/lan-remote-computer-use.md）。
   */
  computerUseCapability?: boolean;
```

2. `createServerInfo`（:281-296）capabilities 改为：

```ts
    capabilities: {
      desktopContinuous: true,
      websocketRpc: true,
      ...(options.computerUseCapability !== undefined
        ? { computerUse: options.computerUseCapability }
        : {}),
    },
```

`packages/desktop/src/host/lanAccessServer.ts` `createHttpServer(...)` 调用（:232-243）追加 option（该文件 :247-254 已用 `process.platform`，同一进程环境）：

```ts
    // LAN 对端是完整 desktop（desktop-local authority）。能力位 = 平台 win/mac && 打包层
    // kill-switch 关（isModeCuaInternalFeatureEnabled 为「只能关不能开」语义）。
    // 插件启用态是 workspace 级动态值，不进设备位（docs/specs/lan-remote-computer-use.md）。
    computerUseCapability:
      (process.platform === "win32" || process.platform === "darwin") &&
      isModeCuaInternalFeatureEnabled(process.env),
```

import `isModeCuaInternalFeatureEnabled` from `@mode/shared`（按 Step 1 确认的导出名）。

- [ ] **Step 7: 跑测试确认通过**

Run: `pnpm --filter @mode/shared test && pnpm --filter @mode/server test`
Expected: PASS（共 6 例新增）。

- [ ] **Step 8: 提交**

```bash
git add packages/shared/src/server-remote.ts packages/shared/test/server-remote-capabilities.test.ts packages/server/src/http.ts packages/server/test/lan-server-info-computer-use.test.ts packages/desktop/src/host/lanAccessServer.ts
git commit -m "feat(cua): LAN server-info 声明设备级 computerUse 能力位"
```

---

### Task 2: 能力位经 descriptor → attachment 参数 → renderer store

**Files:**
- Modify: `packages/shared/src/validation.ts:174-185`（`windowHostRemoteWorkspaceDescriptorSchema`）
- Modify: `packages/desktop/src/host/index.ts:178-183`（`HostRemoteConnectionCapabilities`）、`:1693-1697`（LAN handle capabilities）、`:3039-3043`（`RemoteWorkspaceConnected` postMessage）
- Modify: `packages/desktop/src/main/desktopRemoteSessions.ts:233-241`（attach payload）
- Modify: `packages/desktop/src/preload/index.ts:769-793`（转发）
- Modify: `packages/desktop/src/renderer/src/remoteWorkspaceServicePortBridge.ts:3-27`
- Modify: `packages/desktop/src/renderer/src/main.tsx:203-224`
- Modify: `packages/ui/src/store/remoteWorkspaceSessionStore.ts:7-12`
- Test: `packages/shared/test/window-host-descriptor-capabilities.test.ts`（新）

**Interfaces:**
- Consumes: T1 的 `connection.info.capabilities.computerUse`（`LanRemoteConnection.info`，`lanRemoteAttach.ts:184-188`）。
- Produces:
  - `WindowHostRemoteWorkspaceDescriptor.capabilities?: { computerUse: boolean }`（strict schema）
  - `RemoteWorkspaceSession.capabilities?: { computerUse?: boolean }`（导出 type `RemoteWorkspaceSessionCapabilities`）
  - attachment 注册消息 payload 新可选字段 `capabilities`

- [ ] **Step 1: 写 descriptor schema 失败测试**

新建 `packages/shared/test/window-host-descriptor-capabilities.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { windowHostRemoteWorkspaceDescriptorSchema } from "../src/validation.js";

// 能力位经 host→main 的 descriptor 消息下发；strict schema 下新字段必须显式建模，
// 且旧消息（无 capabilities）必须继续解析（同进程版本一致，但保持 schema 前向可读）。
const baseDescriptor = {
  remoteSessionId: "rs-1",
  target: { kind: "lan" as const, host: "192.168.1.10", port: 3030 },
  workspacePath: "/tmp/ws",
  workspaceIdentity: "remote:lan:192.168.1.10:3030:/tmp/ws",
  generation: 1,
};

test("descriptor 无 capabilities 仍解析通过", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse(baseDescriptor);
  assert.equal(parsed.success, true);
});

test("descriptor 携带 computerUse 能力位解析保留", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse({
    ...baseDescriptor,
    capabilities: { computerUse: true },
  });
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities?.computerUse, true);
});

test("capabilities.computerUse 非布尔解析失败（fail closed）", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse({
    ...baseDescriptor,
    capabilities: { computerUse: 1 },
  });
  assert.equal(parsed.success, false);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/shared test`
Expected: FAIL（strict schema 拒绝未知键 `capabilities` → 用例 2 失败；用例 3 反而绿——记录实际结果，以用例 2 为红灯）。

- [ ] **Step 3: 实现 descriptor schema**

`packages/shared/src/validation.ts` 在 `windowHostRemoteWorkspaceDescriptorSchema`（:174-185）**之前**加：

```ts
/** 远端连接的设备能力位（LAN 握手 server-info.capabilities 投影，strict：未知键拒绝）。 */
export const windowHostRemoteWorkspaceCapabilitiesSchema = z
  .object({
    computerUse: z.boolean(),
  })
  .strict();
export type WindowHostRemoteWorkspaceCapabilities = z.infer<
  typeof windowHostRemoteWorkspaceCapabilitiesSchema
>;
```

descriptor schema 内追加：

```ts
    capabilities: windowHostRemoteWorkspaceCapabilitiesSchema.optional(),
```

- [ ] **Step 4: 跑 shared 测试确认通过**

Run: `pnpm --filter @mode/shared test`
Expected: PASS。

- [ ] **Step 5: host 侧填能力位**

`packages/desktop/src/host/index.ts`：

1. `HostRemoteConnectionCapabilities`（:178-183）加字段：

```ts
interface HostRemoteConnectionCapabilities {
  browserRecordingUploader?: Pick<IRemoteBackend, "upload">;
  remoteMediaPreviewFactory?: (
    scope: Extract<WindowHostAttachmentScope, { kind: "remote" }>,
  ) => RemoteMediaPreviewProxy;
  /** LAN 握手声明的设备级电脑控制能力（对端 server-info.capabilities.computerUse）。 */
  computerUse?: boolean;
}
```

2. LAN handle `capabilities`（:1693-1697，`connection` 在该作用域可用，`info` 目前无人消费）：

```ts
    capabilities: {
      ...(remoteMediaPreviewFactory ? { remoteMediaPreviewFactory } : {}),
      // 首次收口消费 connection.info：设备能力位（旧对端缺键 → 不声明 → 全链路按不支持降级）。
      ...(typeof connection.info.capabilities.computerUse === "boolean"
        ? { computerUse: connection.info.capabilities.computerUse }
        : {}),
    },
```

3. `RemoteWorkspaceConnected` postMessage（:3039-3043）前 enrich（新增局部 helper，放在该 `if (msg.type === HostMessageTypes.ConnectRemoteWorkspace)` 分支上方的进程作用域）：

```ts
function withComputerUseCapability(
  descriptor: WindowHostRemoteWorkspaceDescriptor,
): WindowHostRemoteWorkspaceDescriptor {
  if (!descriptor.workspaceIdentity || !descriptor.workspacePath) return descriptor;
  try {
    const computerUse = windowRemoteConnectionRegistry.resolveScopedCapabilities({
      kind: "remote",
      remoteSessionId: descriptor.remoteSessionId,
      workspacePath: descriptor.workspacePath,
      workspaceIdentity: descriptor.workspaceIdentity,
    })?.computerUse;
    return typeof computerUse === "boolean"
      ? { ...descriptor, capabilities: { computerUse } }
      : descriptor;
  } catch (error) {
    // 能力位只是 UI 判定输入；scope 解析失败不得让已建立的连接回退为 ConnectFailed。
    logger.warn("failed to resolve remote computerUse capability", {
      remoteSessionId: descriptor.remoteSessionId,
      error: error instanceof Error ? error.message : String(error),
    });
    return descriptor;
  }
}
```

postMessage 处改为 `descriptor: withComputerUseCapability(descriptor),`。

- [ ] **Step 6: main → preload → renderer 透传**

1. `packages/desktop/src/main/desktopRemoteSessions.ts` `attachRendererPort`（:233-241）`win.webContents.postMessage` payload 追加：

```ts
            ...(descriptor.capabilities ? { capabilities: descriptor.capabilities } : {}),
```

2. `packages/desktop/src/preload/index.ts`（:769-793）payload 内联类型加 `capabilities?: { computerUse?: boolean };`，`window.postMessage` 对象追加 `capabilities: payload.capabilities,`。

3. `packages/desktop/src/renderer/src/remoteWorkspaceServicePortBridge.ts`：

```ts
export interface RemoteWorkspaceSessionCapabilities {
  computerUse?: boolean;
}

export interface RemoteWorkspaceServicePortRegistration {
  attachmentId: string;
  port: MessagePort;
  sessionId: string;
  target: RemoteTarget;
  capabilities?: RemoteWorkspaceSessionCapabilities;
}
```

parse 函数（:10-27）在必填校验后追加：

```ts
  const rawCapabilities = "capabilities" in event.data ? event.data.capabilities : null;
  const capabilities: RemoteWorkspaceSessionCapabilities | undefined =
    rawCapabilities && typeof rawCapabilities === "object"
      ? {
          ...(typeof (rawCapabilities as { computerUse?: unknown }).computerUse === "boolean"
            ? { computerUse: (rawCapabilities as { computerUse: boolean }).computerUse }
            : {}),
        }
      : undefined;
  return {
    attachmentId,
    port,
    sessionId,
    target,
    ...(capabilities ? { capabilities } : {}),
  };
```

4. `packages/desktop/src/renderer/src/main.tsx` `registerRemoteWorkspaceServicePort`（:214-220）`registerRemoteWorkspaceSession` 追加：

```ts
    ...(params.capabilities ? { capabilities: params.capabilities } : {}),
```

5. `packages/ui/src/store/remoteWorkspaceSessionStore.ts`：

```ts
export interface RemoteWorkspaceSessionCapabilities {
  computerUse?: boolean;
}

export interface RemoteWorkspaceSession {
  sessionId: string;
  target?: RemoteTarget;
  services: IServiceAccessor;
  /** 对端设备能力位（LAN server-info 投影）；旧对端无此字段。 */
  capabilities?: RemoteWorkspaceSessionCapabilities;
  dispose?: (reason?: Error) => void;
}
```

- [ ] **Step 7: 验证**

Run: `pnpm typecheck && pnpm --filter @mode/shared test && pnpm lint`
Expected: 全绿。electron main/host/renderer 链路无单测入口，由 typecheck + T4 的 UI 测试 + 手动实机清单（T9）覆盖——报告里如实注明。

- [ ] **Step 8: 提交**

```bash
git add packages/shared/src/validation.ts packages/shared/test/window-host-descriptor-capabilities.test.ts packages/desktop/src/host/index.ts packages/desktop/src/main/desktopRemoteSessions.ts packages/desktop/src/preload/index.ts packages/desktop/src/renderer/src/remoteWorkspaceServicePortBridge.ts packages/desktop/src/renderer/src/main.tsx packages/ui/src/store/remoteWorkspaceSessionStore.ts
git commit -m "feat(cua): LAN 设备能力位经 descriptor 下发 renderer store"
```

---

### Task 3: 对端 helper admission 对 `remote:lan:` identity 放行

**Files:**
- Modify: `packages/services/src/cuaEnablement.ts`
- Modify: `packages/services/src/node.ts:746-765`（谓词 JSDoc）、`:1745-1755`（admission 调用点）
- Test: `packages/services/test/cua-helper-admission.test.ts`（新）

**Interfaces:**
- Consumes: `parseRemoteWorkspaceIdentity` / `isRemoteWorkspaceIdentity`（`packages/shared/src/remote-workspace-identity.ts:85/:143`，parse 返回 `{ kind: "ssh"|"wsl"|"docker"|"lan", ... } | null`）。
- Produces: `isCuaExcludedRemoteWorkspaceIdentity(identity?: string | null): boolean`（cuaEnablement.ts 导出）；`shouldCreateDefaultCuaProductHelper` 的 `hasRemoteWorkspaceIdentity` 输入语义改为「按 CUA 规则排除的 remote identity」。

- [ ] **Step 1: 写失败测试**

新建 `packages/services/test/cua-helper-admission.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

const { isCuaExcludedRemoteWorkspaceIdentity } = await import("../src/cuaEnablement.js");
const { shouldCreateDefaultCuaProductHelper } = await import("../src/node.js");

// LAN 对端是完整 desktop 自己（desktop-local authority），LAN 挂接工作区的 helper
// 落在对端本机 = 操作对端自己的物理屏幕，不构成跨机器越权（docs/specs/lan-remote-computer-use.md）。
// ssh/wsl/docker 与「任何非空 remote identity」维持既有排除语义。
test("identity 排除判定：lan 放行、ssh/wsl/docker 排除、非 remote 串仍排除", () => {
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity(undefined), false);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity(""), false);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:lan:192.168.1.5:3030:/tmp/ws"), false);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:ssh:host:22:/tmp/ws"), true);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:wsl:Ubuntu:/tmp/ws"), true);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:docker:abc:/tmp/ws"), true);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("some-local-identity"), true);
});

test("shouldCreateDefaultCuaProductHelper：lan identity + desktop-local 放行", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      serviceAuthorityMode: "desktop-local",
      hasRemoteWorkspaceIdentity: isCuaExcludedRemoteWorkspaceIdentity(
        "remote:lan:192.168.1.5:3030:/tmp/ws",
      ),
      hasInjectedResolver: false,
      hasBuiltInCuaPlugin: true,
    }),
    true,
  );
});

test("shouldCreateDefaultCuaProductHelper：ssh identity 维持排除；authority/插件/resolver 门不变", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      serviceAuthorityMode: "desktop-local",
      hasRemoteWorkspaceIdentity: isCuaExcludedRemoteWorkspaceIdentity(
        "remote:ssh:host:22:/tmp/ws",
      ),
      hasInjectedResolver: false,
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      serviceAuthorityMode: "desktop-attached-remote",
      hasRemoteWorkspaceIdentity: false,
      hasInjectedResolver: false,
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      serviceAuthorityMode: "desktop-local",
      hasRemoteWorkspaceIdentity: false,
      hasInjectedResolver: true,
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      serviceAuthorityMode: "desktop-local",
      hasRemoteWorkspaceIdentity: false,
      hasInjectedResolver: false,
      hasBuiltInCuaPlugin: false,
    }),
    false,
  );
});
```

注意：identity 构造以 `remote-workspace-identity.ts:58-78` 实际格式为准；若 `parseRemoteWorkspaceIdentity` 对上述样例返回 null（段数/路径规则不符），先用该文件的 `buildRemoteWorkspaceIdentity(path, target)` 生成样例再断言，**不得**放松断言。

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/services test`
Expected: FAIL（`isCuaExcludedRemoteWorkspaceIdentity` 不存在）。

- [ ] **Step 3: 实现 helper**

`packages/services/src/cuaEnablement.ts` 追加（import `parseRemoteWorkspaceIdentity`、`isRemoteWorkspaceIdentity` from `@mode/shared`）：

```ts
/**
 * CUA helper admission 的 remote identity 排除判定（唯一口径，见 docs/specs/lan-remote-computer-use.md）：
 * - 空 identity：非 remote，不排除；
 * - remote:lan：对端是完整 desktop 自己（desktop-local authority），helper 落在对端本机
 *   操作对端自己的屏幕，放行；
 * - ssh/wsl/docker：对端没有 desktop/Helper，排除；
 * - 无法解析的非空串：保持历史「有 identity 即挡」语义，排除（fail closed）。
 */
export function isCuaExcludedRemoteWorkspaceIdentity(identity?: string | null): boolean {
  const trimmed = identity?.trim();
  if (!trimmed) return false;
  if (!isRemoteWorkspaceIdentity(trimmed)) return true;
  return parseRemoteWorkspaceIdentity(trimmed)?.kind !== "lan";
}
```

- [ ] **Step 4: 接线 admission 调用点**

`packages/services/src/node.ts:1745-1755`：

```ts
      isAdmitted: () =>
        shouldCreateDefaultCuaProductHelper({
          serviceAuthorityMode: options?.serviceAuthorityMode,
          // lan identity 不再按「远端」排除：对端 host 自身是 desktop-local authority，
          // helper 与操作指示都落在对端物理桌面（docs/specs/lan-remote-computer-use.md）。
          hasRemoteWorkspaceIdentity: isCuaExcludedRemoteWorkspaceIdentity(
            context?.workspaceIdentity,
          ),
          hasInjectedResolver: Boolean(options?.cuaProductMcpServerResolver),
          hasBuiltInCuaPlugin: isCuaEnabledForContext(context),
        }),
```

同步更新 `shouldCreateDefaultCuaProductHelper` JSDoc（:746-752）：`hasRemoteWorkspaceIdentity` 语义 = `isCuaExcludedRemoteWorkspaceIdentity` 的结果，lan 已由该口径放行。

- [ ] **Step 5: 排查其它调用点**

Run: `grep -rn "hasRemoteWorkspaceIdentity\|shouldCreateDefaultCuaProductHelper" packages apps --include=*.ts --include=*.mts --include=*.mjs`
Expected: 仅 node.ts 调用点 + 本测试（若有其它调用点/断言旧语义的测试，逐个改为经 `isCuaExcludedRemoteWorkspaceIdentity` 计算，并在提交信息说明）。

- [ ] **Step 6: 跑测试确认通过**

Run: `pnpm --filter @mode/services test`
Expected: PASS（新增 3 例 + 既有 `cua-enablement.test.ts` 全绿）。

- [ ] **Step 7: 提交**

```bash
git add packages/services/src/cuaEnablement.ts packages/services/src/node.ts packages/services/test/cua-helper-admission.test.ts
git commit -m "feat(cua): helper admission 对 remote:lan identity 放行，ssh/wsl/docker 维持排除"
```

---

### Task 4: UI 四门改判（availability / composer / 设置页 / 插件页）

**Files:**
- Modify: `packages/ui/src/settings/computerUseAvailability.ts`
- Create: `packages/ui/src/hooks/useRemoteWorkspaceComputerUseCapability.ts`
- Modify: `packages/ui/src/hooks/useCuaComposerEntry.ts:56-62、:81、:132-145`
- Modify: `packages/ui/src/settings/ComputerUseSection.tsx:77-103、:685-700`
- Modify: `packages/ui/src/settings/PluginsSection.tsx:283-297、:641-666`
- Test: `packages/ui/test/computerUseAvailabilityLan.test.ts`（新）

**Interfaces:**
- Consumes: T2 的 `RemoteWorkspaceSession.capabilities`、store selector（`useRemoteWorkspaceSessionStore`、`sessionIdByWorkspaceIdentity`）。
- Produces:
  - `ComputerUseAvailabilityKind` 新增 `"remote-lan"`；`ComputerUseAvailabilityInput` 新增 `lanComputerUseCapable?: boolean`
  - `useRemoteWorkspaceComputerUseCapability(remoteSessionId?, workspaceIdentity?): boolean | undefined`（undefined = 旧对端/未注册；true/false = 新对端声明）

- [ ] **Step 1: 写 availability 失败测试**

新建 `packages/ui/test/computerUseAvailabilityLan.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

const { resolveComputerUseAvailability } = await import(
  "../src/settings/computerUseAvailability.js"
);

const lanTarget = { kind: "lan" as const, host: "192.168.1.5", port: 3030 };
const lanIdentity = "remote:lan:192.168.1.5:3030:/tmp/ws";

test("LAN + 能力位 true → remote-lan supported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true, remoteTarget: lanTarget, lanComputerUseCapable: true }),
    { kind: "remote-lan", supported: true },
  );
});

test("LAN 无能力位（旧对端）→ remote-lan unsupported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true, remoteTarget: lanTarget }),
    { kind: "remote-lan", supported: false },
  );
});

test("LAN 能力位 false（对端 kill-switch 关）→ unsupported", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true, remoteTarget: lanTarget, lanComputerUseCapable: false }),
    { kind: "remote-lan", supported: false },
  );
});

test("仅 identity 为 remote:lan（历史恢复无 target）也走 remote-lan 分支", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true, workspaceIdentity: lanIdentity, lanComputerUseCapable: true }),
    { kind: "remote-lan", supported: true },
  );
});

test("ssh 维持 remote-ssh unsupported（回归）", () => {
  assert.deepEqual(
    resolveComputerUseAvailability({
      isDesktop: true,
      isWindowsDesktop: true,
      remoteTarget: { kind: "ssh", host: "h", port: 22, username: "u" },
      lanComputerUseCapable: true, // 即使误传能力位也不得放行 ssh
    }),
    { kind: "remote-ssh", supported: false },
  );
});

test("本机与 Web、Linux 分支不变（回归）", () => {
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true, isWindowsDesktop: true }), { kind: "local-windows", supported: true });
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true, isMacDesktop: true }), { kind: "local-macos", supported: true });
  assert.deepEqual(resolveComputerUseAvailability({ isDesktop: true }), { kind: "local-linux", supported: false });
  assert.deepEqual(resolveComputerUseAvailability({}), { kind: "web", supported: false });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm exec tsx --test packages/ui/test/computerUseAvailabilityLan.test.ts`
Expected: FAIL（无 `remote-lan` kind、无新输入 → 用例 1-4 失败；回归例绿）。

- [ ] **Step 3: 实现 availability**

`packages/ui/src/settings/computerUseAvailability.ts`：

1. kind union（:4-12）加 `"remote-lan"`；Input（:19-26）加 `lanComputerUseCapable?: boolean;`；import `parseRemoteWorkspaceIdentity`。
2. `isRemote` 分支（:41-54）在 `remoteKind` 计算前插入：

```ts
    // LAN：完整 desktop 对端，支持与否由握手能力位决定（docs/specs/lan-remote-computer-use.md）。
    // 仅按 identity kind 判定，能力位缺省/为 false 时维持 unsupported。
    const identityKind = workspaceIdentity?.trim()
      ? parseRemoteWorkspaceIdentity(workspaceIdentity.trim())?.kind
      : undefined;
    if (remoteTarget?.kind === "lan" || identityKind === "lan") {
      return { kind: "remote-lan", supported: lanComputerUseCapable === true };
    }
```

- [ ] **Step 4: 新 hook**

新建 `packages/ui/src/hooks/useRemoteWorkspaceComputerUseCapability.ts`：

```ts
import { useRemoteWorkspaceSessionStore } from "@/store/remoteWorkspaceSessionStore.js";

/**
 * 当前 workspace 的 LAN 对端电脑控制能力位（attachment 元数据投影）。
 * - undefined：无在册 session 或旧对端未声明（按不支持处理）；
 * - true/false：新对端声明的设备能力（true 才可判定为支持）。
 * ssh/wsl/docker 永不填该位（LAN handle 之外不注入）。
 */
export function useRemoteWorkspaceComputerUseCapability(
  remoteSessionId?: string | null,
  workspaceIdentity?: string | null,
): boolean | undefined {
  return useRemoteWorkspaceSessionStore((state) => {
    const sessionId =
      remoteSessionId?.trim() ||
      (workspaceIdentity?.trim()
        ? state.sessionIdByWorkspaceIdentity[workspaceIdentity.trim()]
        : undefined);
    if (!sessionId) return undefined;
    return state.sessionsById[sessionId]?.capabilities?.computerUse;
  });
}
```

- [ ] **Step 5: composer 入口改判**

`packages/ui/src/hooks/useCuaComposerEntry.ts`：

1. hook 顶部（platform/services 之后）加：

```ts
  const remoteComputerUse = useRemoteWorkspaceComputerUseCapability(remoteSessionId, workspaceIdentity);
```

2. `isLocalWorkspace`（:60-62）下方加，并改注释：

```ts
  // 与 ComputerUseSection 同口径的 workspace 能力判定：本机 workspace，或 LAN 对端
  // 声明了 computerUse 设备能力位（此时 CUA 操作的是对端自己的物理屏幕，见
  // docs/specs/lan-remote-computer-use.md）。ssh/wsl/docker/手机远控不填该位，维持关闭。
  const workspaceCapable = isLocalWorkspace || remoteComputerUse === true;
```

3. `platformSupported`（:81）改为 `(macLocalDesktop || windowsLocalDesktop) && workspaceCapable;`（update 其所在 effect 依赖数组：加 `workspaceCapable`）。
4. `resolveCuaComposerEntryView`（:132-145）两个平台输入改为：

```ts
        macLocalDesktop: macLocalDesktop && workspaceCapable,
        windowsLocalDesktop: windowsLocalDesktop && workspaceCapable,
```

`permissionServiceAvailable` 暂保持 `Boolean(services.cuaPermissionService)`（T5 完成后 remote pane 的该值自然变为对端服务）。

- [ ] **Step 6: 设置页与插件页改判**

`ComputerUseSection.tsx` 与 `PluginsSection.tsx`（先读两个文件的 `isLocalWorkspace`/`availability` 组装区段）：

1. 各自加：

```ts
  const lanComputerUseCapable = useRemoteWorkspaceComputerUseCapability(remoteSessionId, workspaceIdentity);
```

2. 传入 `resolveComputerUseAvailability({ ..., lanComputerUseCapable })`。
3. 将「本机 workspace 才可配 CUA」的门槛变量（ComputerUseSection 的 `isLocalWorkspace` 参与 `supportsComputerUseSettings`/unsupported 卡判定处、PluginsSection :283-297 同型判定）改为：

```ts
  const workspaceCapableForComputerUse =
    isLocalWorkspace || lanComputerUseCapable === true;
```

并把该变量用于 CUA 分区/插件分组的「是否按支持渲染」判定；`isComputerUseRemoteOrLinux(availability)` 已由 `supported` 自动正确（`remote-lan` unsupported → true → 走既有 remote 文案，无需新 key）。

4. **mac 权限行对端归属**：读 `ComputerUseSection` 权限行（`supportsLocalMacWorkspace`/`cuaPermissionService` 相关区段）。remote pane（`remoteSessionId` 存在）时，权限行渲染前提从「本机平台」改为「`status.available === true`」（对端 win/linux 的 `getStatus` 返回 `available:false` → 行隐藏；对端 mac 返回真实状态 → 行显示且查询走 T5 覆盖后的对端服务）。本机 pane 逻辑不动。

- [ ] **Step 7: 跑测试确认通过**

Run: `pnpm exec tsx --test packages/ui/test/*.test.ts`
Expected: PASS（新 6 例 + 既有 `settingsComputerUseVisibility.test.ts` 全绿）。

- [ ] **Step 8: 验证与提交**

Run: `pnpm typecheck && pnpm lint`
Expected: 绿。

```bash
git add packages/ui/src/settings/computerUseAvailability.ts packages/ui/src/hooks/useRemoteWorkspaceComputerUseCapability.ts packages/ui/src/hooks/useCuaComposerEntry.ts packages/ui/src/settings/ComputerUseSection.tsx packages/ui/src/settings/PluginsSection.tsx packages/ui/test/computerUseAvailabilityLan.test.ts
git commit -m "feat(cua): UI 四门按 LAN 能力位放行，ssh/wsl/docker 维持不支持"
```

---

### Task 5: 权限服务透传与 remote pane 归属

**Files:**
- Modify: `packages/desktop/src/host/remoteWorkspaceServiceCollection.ts:342-401`（注册链之后追加）
- Modify: `packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts`
- Modify: `packages/desktop/src/renderer/src/main.tsx:203-224`（调用处传 capabilities）
- Test: `packages/desktop/tests/remote-workspace-session-services.test.mts`（新）

**Interfaces:**
- Consumes: `IServiceAccessor.cuaPermissionService?`（`accessor.ts:61-62` 可选）；LAN `connection.services` 来自 `lanRemoteAttach` 的 `RemoteServiceAccess`（无条件构造该代理，`packages/client/src/remoteServiceAccess.ts:141-143`），对端 `createLocalServices` 恒注册 `ICuaPermissionService`（`node.ts:2485`）。
- Produces: `createRemoteWorkspaceServiceCollection` 在 `connectionServices.cuaPermissionService` 存在时注册该 channel；`buildRemoteWorkspaceSessionServices(base, remote, options?: { capabilities?: { computerUse?: boolean } })` 在 `typeof capabilities?.computerUse === "boolean"` 时用 remote 覆盖 `cuaPermissionService` 键。

- [ ] **Step 1: 写 merge 失败测试**

新建 `packages/desktop/tests/remote-workspace-session-services.test.mts`（import 风格对齐 `lan-attach-initialize.test.mjs` 的 fixture/tsx 约定与兄弟 `.mts` 测试；若 `.mts` 直接 import 源码在 `node --test` 下不可行，改用 `spawn(process.execPath, ["--import", "tsx", fixture])` 夹具模式，断言内容不变）：

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildRemoteWorkspaceSessionServices } from "../src/renderer/src/remoteWorkspaceSessionServices.js";

function makeAccessor(overrides: Record<string, unknown> = {}) {
  return { cuaPermissionService: { kind: "base" }, ...overrides } as never;
}

// 权限服务归属：LAN 新对端（capabilities.computerUse 为布尔，键出现即新版对端且其
// createLocalServices 恒注册该 channel）→ 用对端服务；旧对端/ssh → 维持本机 base 回落
// （docs/specs/lan-remote-computer-use.md「权限服务透传与归属」）。
test("LAN 新对端：cuaPermissionService 归属覆盖为对端服务", () => {
  const base = makeAccessor();
  const remote = makeAccessor({ cuaPermissionService: { kind: "remote" } });
  const merged = buildRemoteWorkspaceSessionServices(base, remote, {
    capabilities: { computerUse: true },
  });
  assert.deepEqual(merged.cuaPermissionService, { kind: "remote" });
});

test("能力位为 false（新对端 kill-switch 关）仍归属对端：状态查询是对端事实", () => {
  const merged = buildRemoteWorkspaceSessionServices(
    makeAccessor(),
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
    { capabilities: { computerUse: false } },
  );
  assert.deepEqual(merged.cuaPermissionService, { kind: "remote" });
});

test("无 capabilities（旧对端 / ssh）维持本机 base 回落", () => {
  const base = makeAccessor();
  const merged = buildRemoteWorkspaceSessionServices(
    base,
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
    {},
  );
  assert.deepEqual(merged.cuaPermissionService, { kind: "base" });
  const mergedNoOptions = buildRemoteWorkspaceSessionServices(
    base,
    makeAccessor({ cuaPermissionService: { kind: "remote" } }),
  );
  assert.deepEqual(mergedNoOptions.cuaPermissionService, { kind: "base" });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test packages/desktop/tests/remote-workspace-session-services.test.mts`
Expected: FAIL（签名无第三参 → 类型/运行错误；键未覆盖 → 断言失败）。

- [ ] **Step 3: 实现远端服务集合条件注册**

`packages/desktop/src/host/remoteWorkspaceServiceCollection.ts`：import `ICuaPermissionService`（from `@mode/services`）。在 `const services = new ServiceCollection().register(...)` 链（:342-401）结束之后、函数 return 之前加：

```ts
  // LAN 远端面板的 OS 权限状态必须是对端事实：对端 connectionServices 提供时透传，
  // 不入 legacyRemoteWorkspaceRpcContract 必选白名单（ssh/wsl/docker 的 stdio host
  // 不提供该服务，入白名单会断链）。依据 docs/specs/lan-remote-computer-use.md。
  if (params.connectionServices.cuaPermissionService) {
    services.register(ICuaPermissionService, params.connectionServices.cuaPermissionService);
  }
```

确认注册链产出的变量名是 `services`（以实际代码为准），链式 const 若不能后补注册，改为先建 `const services = new ServiceCollection();` 再 `services.register(...)` 逐条（保持既有顺序与注释）。

- [ ] **Step 4: 实现 renderer merge 归属**

`packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts`：

```ts
import type { IServiceAccessor } from "@mode/services";

export interface RemoteWorkspaceSessionMergeOptions {
  /** attachment 元数据里的对端能力位；键出现即新版对端（其 host 恒注册 cuaPermissionService channel）。 */
  capabilities?: { computerUse?: boolean };
}

export function buildRemoteWorkspaceSessionServices(
  baseServices: IServiceAccessor,
  remoteServices: IServiceAccessor,
  options?: RemoteWorkspaceSessionMergeOptions,
): IServiceAccessor {
  const merged: IServiceAccessor = {
    ...baseServices,
    // ……既有覆盖键全部保持原样……
  };
  // 权限服务归属：LAN 新对端时用对端代理（getStatus/restartHelper 都是对端机器事实）；
  // 旧对端未声明能力键 → 该 channel 可能不存在，维持本机 base 回落避免悬空代理超时。
  if (
    typeof options?.capabilities?.computerUse === "boolean" &&
    remoteServices.cuaPermissionService
  ) {
    merged.cuaPermissionService = remoteServices.cuaPermissionService;
  }
  return merged;
}
```

（实现时把既有 return 对象体原样移入 `merged` 初始化，不改动任何既有覆盖键与注释。）

`packages/desktop/src/renderer/src/main.tsx` 调用处（:210-213）：

```ts
  const services = buildRemoteWorkspaceSessionServices(
    baseServicesForRemoteSessions,
    remoteServices,
    ...(params.capabilities ? { capabilities: params.capabilities } : {}),
  );
```

- [ ] **Step 5: 跑测试确认通过 + 端到端面**

Run: `node --test packages/desktop/tests/remote-workspace-session-services.test.mts && pnpm typecheck`
Expected: PASS + 绿。

补充核对（写进提交前检查，不新增代码）：
- `grep -rn "createRemoteWorkspaceServiceCollection" packages/desktop/src` → 两个调用点（LAN handle `index.ts:1655`、stdio handle `index.ts:1766`）。stdio 的 `connectionServices.cuaPermissionService` 若为悬空代理也会被注册——但 renderer merge 对无 capabilities 的 stdio 连接不覆盖该键，channel 暸不会被调用；确认无第三方直接消费该 channel 的调用点：`grep -rn "cuaPermissionService" packages/ui/src packages/desktop/src/renderer`。

- [ ] **Step 6: 提交**

```bash
git add packages/desktop/src/host/remoteWorkspaceServiceCollection.ts packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts packages/desktop/src/renderer/src/main.tsx packages/desktop/tests/remote-workspace-session-services.test.mts
git commit -m "feat(cua): 权限服务经远端服务集合透传，LAN 新对端归属对端服务"
```

---

### Task 6: 权限观察事件面放行（base 过滤 + relay scope 门）

**Files:**
- Modify: `packages/services/src/mode-agent/modeAgent.ts:189-190、:820`（签名加可选 params）
- Modify: `packages/services/src/mode-agent/modeAgentService.ts:2018-2044`（过滤条件）
- Modify: `packages/services/src/mode-agent/modeAgentConnectionScope.ts:890-897`（scope 门）
- Test: `packages/services/test/cua-permission-observation-gate.test.ts`（新）

**Interfaces:**
- Consumes: `readTrustedModeAgentV4Connection` / `withTrustedConnection`（`modeAgentConnectionScope.ts:71-109` 导出/私有，`withTrustedConnection` 若未导出则保持文件内使用）；`ModeAgentV4ConnectionContext`（`connectionId; clientMode; role?`）。
- Produces:
  - `shouldForwardCuaPermissionObservation(workspace): boolean`（modeAgentService.ts 导出纯函数）
  - `IModeAgentService.onDynamicCuaPermissionObservation(params?: unknown) => Event<ModeAgentCuaPermissionObservation>`
  - scope 门放行形态：`terminal-client+desktop-continuous`（本地）与 `trusted-host-relay+下游 desktop-continuous`（LAN relay）；replayable 下游恒拒绝。

- [ ] **Step 1: 写失败测试**

新建 `packages/services/test/cua-permission-observation-gate.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

const { shouldForwardCuaPermissionObservation } = await import(
  "../src/mode-agent/modeAgentService.js"
);
const { createModeAgentConnectionScope } = await import(
  "../src/mode-agent/modeAgentConnectionScope.js"
);
import type { IModeAgentService } from "../src/mode-agent/modeAgent.js";

// ---- base 过滤（对端 host 进程内）----
test("base 过滤：本机与 remote:lan 放行；ssh/wsl/docker 与 remoteSessionId 丢弃", () => {
  assert.equal(shouldForwardCuaPermissionObservation({}), true);
  assert.equal(
    shouldForwardCuaPermissionObservation({
      workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws",
    }),
    true,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:ssh:h:22:/tmp/ws" }),
    false,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:wsl:Ubuntu:/tmp/ws" }),
    false,
  );
  assert.equal(
    shouldForwardCuaPermissionObservation({ workspaceIdentity: "remote:docker:c:/tmp/ws" }),
    false,
  );
  assert.equal(shouldForwardCuaPermissionObservation({ remoteSessionId: "rs-1" }), false);
});

// ---- scope 门（连接面）----
function makeBase() {
  const calls: unknown[] = [];
  const base = {
    onDynamicCuaPermissionObservation: (params?: unknown) => {
      calls.push(params);
      return { dispose: () => {} };
    },
  } as unknown as IModeAgentService;
  return { base, calls };
}
const continuous = { connectionId: "conn-1", clientMode: "desktop-continuous" as const };

test("scope 门：terminal-client + desktop-continuous 放行并携带 trusted 载体", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, { ...continuous });
  scope.service.onDynamicCuaPermissionObservation();
  assert.equal(calls.length, 1);
  const carrier = calls[0] as {
    __modeTrustedV4Connection?: { clientMode?: string };
  };
  assert.equal(carrier.__modeTrustedV4Connection?.clientMode, "desktop-continuous");
  scope.dispose();
});

test("scope 门：trusted-host-relay + 可信 desktop-continuous 下游放行（LAN 先例）", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation({
    __modeTrustedV4Connection: { connectionId: "host-rpc-1", clientMode: "desktop-continuous" },
  });
  assert.equal(calls.length, 1);
  scope.dispose();
});

test("scope 门：relay 无可信下游 → RpcEvent.None，不落 base", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation(undefined);
  assert.equal(calls.length, 0);
  scope.dispose();
});

test("scope 门：relay 下游是手机 replay → 拒绝", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "server-ws-1",
    clientMode: "desktop-continuous",
    role: "trusted-host-relay",
  });
  scope.service.onDynamicCuaPermissionObservation({
    __modeTrustedV4Connection: { connectionId: "att-1", clientMode: "web-remote-replayable" },
  });
  assert.equal(calls.length, 0);
  scope.dispose();
});

test("scope 门：本连接 clientMode 为 replayable（手机 attachment）恒拒绝", () => {
  const { base, calls } = makeBase();
  const scope = createModeAgentConnectionScope(base, {
    connectionId: "att-1",
    clientMode: "web-remote-replayable",
  });
  scope.service.onDynamicCuaPermissionObservation();
  assert.equal(calls.length, 0);
  scope.dispose();
});
```

注意：`__modeTrustedV4Connection` 字段名以 `readTrustedModeAgentV4Connection`（:71-89）实现为准；`scope.service` 暴露的方法名/可调用形态以 `:229-234` 返回值为准（若返回的是 `service` 包装对象，直接调用即触发门控）。

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/services test`
Expected: FAIL（`shouldForwardCuaPermissionObservation` 不存在；scope 现实现只认 `role === "terminal-client"` → relay 用例失败）。

- [ ] **Step 3: 实现 base 过滤纯函数**

`modeAgentService.ts` 文件作用域（emitter :1132 附近）导出：

```ts
/**
 * CUA 权限观察是否可投递（唯一口径）：remoteSessionId 连接面一律不投；
 * remote identity 中仅 remote:lan 放行——LAN 对端是完整 desktop 自己，权限引导
 * 对应的是对端物理桌面（docs/specs/lan-remote-computer-use.md）。
 * 其余（本机、无法解析的 identity 维持历史语义放行/排除）与既有行为一致。
 */
export function shouldForwardCuaPermissionObservation(workspace: {
  remoteSessionId?: string;
  workspaceIdentity?: string;
}): boolean {
  if (workspace.remoteSessionId) return false;
  const identity = workspace.workspaceIdentity?.trim();
  if (!identity || !isRemoteWorkspaceIdentity(identity)) return true;
  return parseRemoteWorkspaceIdentity(identity)?.kind === "lan";
}
```

通知处理（:2018-2044）条件改为：

```ts
          if (parsed.success && shouldForwardCuaPermissionObservation(workspace)) {
```

（import `parseRemoteWorkspaceIdentity`；`isRemoteWorkspaceIdentity` 已在用。）

- [ ] **Step 4: 实现 scope 门**

1. `modeAgent.ts:820` 签名改为：

```ts
  onDynamicCuaPermissionObservation(params?: unknown): Event<ModeAgentCuaPermissionObservation>;
```

`modeAgentService.ts:5470-5472` 实现无需改（忽略参数）。

2. `modeAgentConnectionScope.ts:890-897` 替换为（先读 `:868-889` telemetry 先例与 `:71-109` 两个 helper 的确切实现，保持 helper 调用形态一致）：

```ts
    onDynamicCuaPermissionObservation(params?: unknown) {
      assertOpen();
      // 权限引导是物理桌面副作用：本地桌面 attachment 与「可信 relay + desktop-continuous
      // 下游」可消费；手机 replay 下游与 replayable 连接恒拒绝。放行形态照
      // onDynamicConversationTelemetryFact 的 relay 先例（同文件上方）。
      const downstream = readTrustedModeAgentV4Connection(params);
      const relayDesktopDownstream =
        role === "trusted-host-relay" && downstream?.clientMode === "desktop-continuous";
      if (
        context.clientMode !== "desktop-continuous" ||
        (role !== "terminal-client" && !relayDesktopDownstream)
      ) {
        return RpcEvent.None;
      }
      // 本地 hop 把本连接的可信 clientMode 打成载体带给下一跳：LAN relay 的对端 scope
      // 正是靠它判定「下游是可信桌面」。base 是真实 service 时该参数被忽略，无副作用。
      return base.onDynamicCuaPermissionObservation(
        withTrustedConnection({}, {
          connectionId: context.connectionId,
          clientMode: context.clientMode,
        }),
      );
    },
```

若 `withTrustedConnection` / `ModeAgentV4ConnectionContext` 的字段约束与字面量不完全一致（如需 `role` 或缺 `workflowRunDeltas` 可选项），按类型定义补齐必填字段；**不得**绕过 trusted 载体直接传 `params`（那会让 relay 门在本地 hop 失去下游信息）。

- [ ] **Step 5: 跑测试确认通过**

Run: `pnpm --filter @mode/services test`
Expected: PASS（新 7 例 + 既有服务测试全绿）。

- [ ] **Step 6: 跨面回归确认**

Run: `pnpm typecheck && pnpm lint`
Expected: 绿。`grep -rn "onDynamicCuaPermissionObservation" packages apps --include=*.ts --include=*.tsx` —— renderer 现有调用 `...(obs)()` 无参形态必须仍编译（params 可选）。

- [ ] **Step 7: 提交**

```bash
git add packages/services/src/mode-agent/modeAgent.ts packages/services/src/mode-agent/modeAgentService.ts packages/services/src/mode-agent/modeAgentConnectionScope.ts packages/services/test/cua-permission-observation-gate.test.ts
git commit -m "feat(cua): 权限观察事件面对 remote:lan 与可信 relay 下游放行"
```

---

### Task 7: `openPermissionOnboarding` 服务管道（host→main，动作落在对端机器）

**Files:**
- Modify: `packages/services/src/cua-permission-broker/cuaPermissionService.ts:51-55`
- Modify: `packages/services/src/node.ts`（options 类型 + cuaPermissionService 实现体 :1902-2087 + `createServices` 注入点）
- Modify: `packages/shared/src/channels.ts`（`HostResponseTypes` 新键、`HostMessageTypes` 新键）
- Modify: `packages/shared/src/validation.ts:1024-1073`（`hostResponseMessageSchema` 新变体 + 结果 schema）
- Modify: `packages/desktop/src/host/index.ts`（opener 实现 + 注入 services options，位置对齐 `cuaOperationStateReporter` 注入 :3288 附近）
- Modify: `packages/desktop/src/main/desktopHostProcess.ts:299-366`（新分支）
- Test: `packages/shared/test/host-cua-onboarding-message.test.ts`（新）

**Interfaces:**
- Consumes: 现成模式 A「requestId + 回包」（`authorizeLocalMediaPreviewPath` `host/index.ts:223-241` ↔ `desktopHostProcess.ts:299-366`）；main 底层 `openCuaPermissionOnboarding`（`desktop/src/main/cuaAccessibilitySettings.ts:504`，options 含 `participantKey?: string`，:29-60）。
- Produces:
  - services 侧 `interface ICuaPermissionService`（`cuaPermissionService.ts`，**不改 vendored `packages/mode-cua/broker.d.ts`**）新增可选方法：
    `openPermissionOnboarding?(workspacePath?: string, workspaceIdentity?: string, options?: { initialPermission?: string; requiredPermissions?: string[] }): Promise<{ success: boolean; error?: string }>`
  - `createServices` options 新增 `cuaPermissionOnboardingOpener?: (options: { initialPermission?: string; requiredPermissions?: string[] }) => Promise<{ success: boolean; error?: string }>`
  - `HostResponseTypes.OpenCuaPermissionOnboardingRequest = "host-open-cua-permission-onboarding-request"`、`HostMessageTypes.OpenCuaPermissionOnboardingResult = "open-cua-permission-onboarding-result"`（命名以相邻键风格为准）
  - main 侧 participantKey 形如 `host:${process.pid}`（不依赖 `event.sender`）

- [ ] **Step 1: 写消息 schema 失败测试**

新建 `packages/shared/test/host-cua-onboarding-message.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { hostResponseMessageSchema } from "../src/validation.js";

// host→main 打开对端 onboarding 的新消息：discriminated union 必须显式登记，
// 否则 main 的 safeParse 静默丢弃（与既有 LocalMediaPreviewPathAuthorizeRequest 同规）。
test("host 请求消息解析保留 requestId 与权限参数", () => {
  const parsed = hostResponseMessageSchema.safeParse({
    type: "host-open-cua-permission-onboarding-request",
    requestId: "req-1",
    initialPermission: "screenRecording",
    requiredPermissions: ["screenRecording", "accessibility"],
  });
  assert.equal(parsed.success, true);
});

test("未知 type 拒绝（fail closed，回归既有语义）", () => {
  const parsed = hostResponseMessageSchema.safeParse({ type: "host-not-a-real-type" });
  assert.equal(parsed.success, false);
});
```

（type 字面量以 Step 3 实际落名替换，两处必须同源——测试与实现引用同一常量。）

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/shared test`
Expected: FAIL（union 无该变体）。

- [ ] **Step 3: 实现 shared 消息面**

1. `packages/shared/src/channels.ts`：在 `HostResponseTypes`（`LocalMediaPreviewPathAuthorizeRequest` :654 附近）加请求键；在 `HostMessageTypes`（`LocalMediaPreviewPathAuthorizeResult` :542 附近）加结果键；键名带中文注释：

```ts
  /** Host → Main：打开本机（对端机器）的 Computer Use 权限引导；LAN 远端面板经此触发。 */
  OpenCuaPermissionOnboardingRequest: "host-open-cua-permission-onboarding-request",
  // HostMessageTypes：
  /** Main → Host：onboarding 打开结果回包。 */
  OpenCuaPermissionOnboardingResult: "open-cua-permission-onboarding-result",
```

2. `packages/shared/src/validation.ts`：仿 `hostLocalMediaPreviewPathAuthorizeRequestResponseSchema`（:1024-1073 一带）加两个 schema 并登记进 `hostResponseMessageSchema` discriminatedUnion；同时在 HostMessageTypes 对应的入站 union（若 main→host 消息也走 schema 校验——查 `desktopHostProcess.ts` 是否校验入站；不校验则只加请求侧）：

```ts
export const hostOpenCuaPermissionOnboardingRequestSchema = z
  .object({
    type: z.literal("host-open-cua-permission-onboarding-request"),
    requestId: nonEmptyStringSchema,
    initialPermission: z.string().optional(),
    requiredPermissions: z.array(z.string()).optional(),
  })
  .strict();

export const hostOpenCuaPermissionOnboardingResultSchema = z
  .object({
    type: z.literal("open-cua-permission-onboarding-result"),
    requestId: nonEmptyStringSchema,
    success: z.boolean(),
    error: z.string().optional(),
  })
  .strict();
```

- [ ] **Step 4: 跑 shared 测试确认通过**

Run: `pnpm --filter @mode/shared test`
Expected: PASS。

- [ ] **Step 5: services 接口与实现**

1. `packages/services/src/cua-permission-broker/cuaPermissionService.ts`（:51-55）：

```ts
export interface ICuaPermissionService extends BrokerICuaPermissionService {
  /**
   * 打开**本机** Computer Use 权限引导（动作落在该 service 所在 host 的主进程）。
   * LAN 远端面板经透传调用时，窗口开在对端机器上——不依赖 renderer preload，
   * 因此不能用 IPlatformService.openCuaPermissionOnboarding（那只会开本机的）。
   */
  openPermissionOnboarding?(
    workspacePath?: string,
    workspaceIdentity?: string,
    options?: { initialPermission?: string; requiredPermissions?: string[] },
  ): Promise<{ success: boolean; error?: string }>;
}

export const ICuaPermissionService = createServiceDescriptor<
  BrokerICuaPermissionService & Pick<ICuaPermissionService, "openPermissionOnboarding">
>(ServiceChannels.CuaPermission);
```

（若 `createServiceDescriptor` 泛型与上述交叉不兼容，以类型编译为准调整——原则：接口类型含可选新方法，descriptor 通道描述符不变、channel 名不变。）

2. `packages/services/src/node.ts`：

- options 类型加 `cuaPermissionOnboardingOpener?: (options: { initialPermission?: string; requiredPermissions?: string[] }) => Promise<{ success: boolean; error?: string }>;`（JSDoc：desktop host 注入；缺省时方法返回 `{success:false,error:"unavailable"}`）。
- cuaPermissionService 实现对象（:1902-2087）加方法，门控与 getStatus 同源（先读 :1902-1945 的 darwin/enablement 判定，保持同一口径）：

```ts
      openPermissionOnboarding: async (_workspacePath, _workspaceIdentity, options) => {
        // 与 getStatus 同口径：未启用直接 fail closed，不无谓拉起主进程引导。
        if (!shouldUseCuaPermissionService({ cuaEnabled: isCuaEnabledForContext() })) {
          return { success: false, error: "cua-disabled" };
        }
        if (!opener) {
          return { success: false, error: "unavailable" };
        }
        return opener({
          ...(options?.initialPermission ? { initialPermission: options.initialPermission } : {}),
          ...(options?.requiredPermissions
            ? { requiredPermissions: options.requiredPermissions }
            : {}),
        });
      },
```

（`opener` = `options?.cuaPermissionOnboardingOpener`；若该实现对象所在作用域的 enablement 判定需要 context/workspace 参数，直接沿用 getStatus 的既有写法逐字对齐。）

- [ ] **Step 6: desktop host 注入 + main 分支**

1. `packages/desktop/src/host/index.ts`：

- 新增 host→main 请求函数（仿 `authorizeLocalMediaPreviewPath` :223-241，pending map 同款）：

```ts
const pendingCuaOnboardingOpens = new Map<
  string,
  { resolve: (value: { success: boolean; error?: string }) => void }
>();

function openCuaPermissionOnboardingViaMain(options: {
  initialPermission?: string;
  requiredPermissions?: string[];
}): Promise<{ success: boolean; error?: string }> {
  if (!parentPort) {
    return Promise.resolve({ success: false, error: "parentPort unavailable" });
  }
  const requestId = randomUUID();
  return new Promise((resolve) => {
    pendingCuaOnboardingOpens.set(requestId, { resolve });
    parentPort.postMessage({
      type: HostResponseTypes.OpenCuaPermissionOnboardingRequest,
      requestId,
      ...(options.initialPermission ? { initialPermission: options.initialPermission } : {}),
      ...(options.requiredPermissions ? { requiredPermissions: options.requiredPermissions } : {}),
    });
    // 主进程不应长时间不回；超时按失败收敛，不悬挂远端面板的授权流。
    setTimeout(() => {
      const pending = pendingCuaOnboardingOpens.get(requestId);
      if (pending) {
        pendingCuaOnboardingOpens.delete(requestId);
        pending.resolve({ success: false, error: "onboarding-open-timeout" });
      }
    }, 15_000).unref?.();
  });
}
```

- 结果回包分派：在 host 侧接收 `parentPort` message 的既有分派处（grep `HostMessageTypes.LocalMediaPreviewPathAuthorizeResult` 的接收点，:336-366 main 发、host 收——找到 host 收侧）加同款分支：删除 pending、resolve `{success, error}`。
- 注入：`createServices(...)` options（`cuaOperationStateReporter` 注入 :3288 附近）加 `cuaPermissionOnboardingOpener: openCuaPermissionOnboardingViaMain,`（按该处平台/条件注释风格，**无条件注入**——方法自身有 enablement 门，非 Windows/macOS 由 main 侧返回失败）。

2. `packages/desktop/src/main/desktopHostProcess.ts`（:299-366 分派内加分支）：

```ts
      if (parsed.data.type === HostResponseTypes.OpenCuaPermissionOnboardingRequest) {
        void openCuaPermissionOnboarding({
          // host 发起没有 event.sender：用 host 进程身份做 participant，main 的
          // participantKey 语义（cuaAccessibilitySettings.ts:29-60）本就支持自定义键。
          participantKey: `host:${child.pid ?? "unknown"}`,
          ...(parsed.data.initialPermission
            ? { initialPermission: parsed.data.initialPermission }
            : {}),
          ...(parsed.data.requiredPermissions
            ? { requiredPermissions: parsed.data.requiredPermissions }
            : {}),
        })
          .then((result) => {
            child.postMessage({
              type: HostMessageTypes.OpenCuaPermissionOnboardingResult,
              requestId: parsed.data.requestId,
              success: result.success,
              ...(result.error ? { error: result.error } : {}),
            });
          })
          .catch((error: unknown) => {
            child.postMessage({
              type: HostMessageTypes.OpenCuaPermissionOnboardingResult,
              requestId: parsed.data.requestId,
              success: false,
              error: error instanceof Error ? error.message : String(error),
            });
          });
        return;
      }
```

import `openCuaPermissionOnboarding` from `./cuaAccessibilitySettings.js`（返回形状以 :504 实际为准，若返回 `CuaAccessibilitySettingsResult`（`success/error` 字段名可能不同），按实际字段映射到 `success/error`，保持消息 schema 不变）。`participantKey` 字段若已被 main IPC handler 强制覆盖（`desktopCuaPermissionIpc.ts:438-451` 用 `sender.id`），此处分支**直接调用底层 `openCuaPermissionOnboarding`**（绕开 ipcMain.handle 层），participantKey 用 `host:${child.pid}`——这就是 Step 1 里「直接调用底层」的原因，不得复用 ipcMain handler。

- [ ] **Step 7: 验证**

Run: `pnpm typecheck && pnpm --filter @mode/shared test && pnpm lint`
Expected: 绿。host↔main 分支无单测入口，由 typecheck + shared schema 测试 + T8/实机清单覆盖（报告注明）。

- [ ] **Step 8: 提交**

```bash
git add packages/services/src/cua-permission-broker/cuaPermissionService.ts packages/services/src/node.ts packages/shared/src/channels.ts packages/shared/src/validation.ts packages/shared/test/host-cua-onboarding-message.test.ts packages/desktop/src/host/index.ts packages/desktop/src/main/desktopHostProcess.ts
git commit -m "feat(cua): ICuaPermissionService.openPermissionOnboarding 经 host→main 打开本机引导"
```

---

### Task 8: 观察附件多源订阅 + 对端路由 + i18n

**Files:**
- Modify: `packages/ui/src/cua-permission/CuaPermissionObservationAttachment.tsx`（:112-208 主体）
- Modify: `packages/ui/src/i18n/locales/zh-CN.ts`、`en-US.ts`、`fa.ts`
- Test: `packages/ui/test/cuaPermissionObservationSource.test.ts`（新，纯函数部分）

**Interfaces:**
- Consumes: T5 的 remote `cuaPermissionService` 归属、T6 的多源事件、T7 的 `openPermissionOnboarding`；`useRemoteWorkspaceSessionStore`（订阅 `sessionsById` 变化）、`resolveRegisteredWorkspaceServices`（`remoteWorkspaceSessionStore.ts:203-222`，按 identity 解析归属 accessor）。
- Produces: 附件组件按「事件来源 accessor」执行 getStatus/restartHelper/openOnboarding；remote 来源走 service 方法 + 对端文案，base 来源维持 `platform.openCuaPermissionOnboarding`。

- [ ] **Step 1: 读现状**

Read: `packages/ui/src/cua-permission/CuaPermissionObservationAttachment.tsx` 全文（尤其 :56-110 的 `cuaPermissionObservationKey`/`runCuaPermissionPrompt` 与 :112-208 effect）、`runCuaPermissionPrompt` 的 `confirm` 文案现有 intl key。本任务不改 `runCuaPermissionPrompt` 签名，只改调用侧的 accessor 与文案选择。

- [ ] **Step 2: 写纯函数失败测试（来源判定）**

在组件文件中抽出并导出纯函数：

```ts
/**
 * 权限观察的归属 accessor：identity 解析到在册 remote session 时用对端服务
 * （LAN 新对端才会有该 session 的 capabilities——T5 已保证归属正确），否则本机 base。
 */
export function resolveCuaPermissionSource(input: {
  baseServices: IServiceAccessor;
  observation: { workspaceIdentity?: string };
  sessionsById: Record<string, RemoteWorkspaceSession>;
  sessionIdByWorkspaceIdentity: Record<string, string>;
}): { services: IServiceAccessor; remote: boolean } {
  const identity = input.observation.workspaceIdentity?.trim();
  const sessionId = identity ? input.sessionIdByWorkspaceIdentity[identity] : undefined;
  const session = sessionId ? input.sessionsById[sessionId] : undefined;
  if (session && typeof session.capabilities?.computerUse === "boolean") {
    return { services: session.services, remote: true };
  }
  return { services: input.baseServices, remote: false };
}
```

新建 `packages/ui/test/cuaPermissionObservationSource.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

const { resolveCuaPermissionSource } = await import(
  "../src/cua-permission/CuaPermissionObservationAttachment.js"
);

const base = { tag: "base" } as never;
const remoteSession = {
  services: { tag: "remote" },
  capabilities: { computerUse: true },
} as never;

test("identity 绑定到 LAN 新对端 session → 对端来源", () => {
  const result = resolveCuaPermissionSource({
    baseServices: base,
    observation: { workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws" },
    sessionsById: { s1: remoteSession as never },
    sessionIdByWorkspaceIdentity: { "remote:lan:192.168.1.5:3030:/tmp/ws": "s1" },
  });
  assert.equal(result.remote, true);
  assert.deepEqual(result.services, { tag: "remote" });
});

test("无绑定 / 旧对端（无 capabilities）→ 本机来源", () => {
  for (const sessionsById of [{}, { s1: { services: { tag: "remote" } } as never }]) {
    const result = resolveCuaPermissionSource({
      baseServices: base,
      observation: { workspaceIdentity: "remote:lan:192.168.1.5:3030:/tmp/ws" },
      sessionsById,
      sessionIdByWorkspaceIdentity: { "remote:lan:192.168.1.5:3030:/tmp/ws": "s1" },
    });
    assert.equal(result.remote, false);
    assert.deepEqual(result.services, base);
  }
});

test("本机 workspace（无 identity）→ 本机来源", () => {
  const result = resolveCuaPermissionSource({
    baseServices: base,
    observation: {},
    sessionsById: {},
    sessionIdByWorkspaceIdentity: {},
  });
  assert.equal(result.remote, false);
});
```

- [ ] **Step 3: 跑测试确认失败**

Run: `pnpm exec tsx --test packages/ui/test/cuaPermissionObservationSource.test.ts`
Expected: FAIL（导出不存在）。

- [ ] **Step 4: 实现组件多源订阅与路由**

`CuaPermissionObservationAttachment.tsx`：

1. 抽出 `resolveCuaPermissionSource`（Step 2 代码，import `RemoteWorkspaceSession` type 与 store）。注意 tsx 测试直接 import 组件文件会连带拉入 React/JSX——若 import 链过重（测试跑不动），把纯函数移到新文件 `packages/ui/src/cua-permission/cuaPermissionObservationSource.ts`，组件与测试都从新文件 import（**推荐，优先执行此拆分**）。
2. effect 重构（保持既有 `handled`/`pending`/`chain`/`runCuaPermissionPrompt` 编排不动）：
   - 订阅源集合 = `[baseServices]` + `useRemoteWorkspaceSessionStore.getState().sessionsById` 每个 session（订阅 store：`useRemoteWorkspaceSessionStore.subscribe` 做差量增删；session 换代时旧 services 退订、新 services 订阅）。
   - base 订阅沿用现有 guard（`platform.openCuaPermissionOnboarding` 存在才订 base）；remote 订阅的 guard = `session.services.cuaPermissionService` 存在 + `typeof modeAgentService.onDynamicCuaPermissionObservation === "function"`（**不**要求 platform 函数——那是本机 preload，对端用不上）。
   - 每条观察闭包持有 `source = { services, remote }`（订阅时已知，或用 `resolveCuaPermissionSource` 二次校验）：
     - `getStatus` / `restartHelper` / `refresh` 一律用 `source.services.cuaPermissionService`（不再用组件顶层 `services.cuaPermissionService`）；
     - `openOnboarding`：`source.remote` 时：

```ts
                openOnboarding: async (required) => {
                  const service = source.services.cuaPermissionService;
                  if (!service?.openPermissionOnboarding) {
                    return { success: false, error: "unavailable" };
                  }
                  return (
                    (await service.openPermissionOnboarding(
                      observation.workspacePath,
                      observation.workspaceIdentity,
                      {
                        initialPermission: required[0],
                        requiredPermissions: required,
                      },
                    )) ?? { success: false, error: "unavailable" }
                  );
                },
```

       否则维持现有 `platform.openCuaPermissionOnboarding` 路径（含 `activeOperationId` 追踪；remote 分支不产生 operationId，cleanup 的 `cancelCuaPermissionOnboarding` 只对 base 分支执行）。
     - `confirm` 文案：`source.remote` 时用新 key（见 Step 5）的 description，其余 title/confirmLabel/cancelLabel 沿用现有 key（描述权限项的文案机器无关）。
   - 订阅的 dispose 汇总进 effect cleanup（含 store unsubscribe）。
3. 顶部整体 guard 调整：组件仍以 base `services.cuaPermissionService` + `platform.openCuaPermissionOnboarding` 决定 **base** 订阅；组件顶层不再因 platform 函数缺失整体 return（否则无本机 onboarding 的平台连远端订阅都拿不到——但注意：Web 平台 `allowRemoteWorkspace=false` 无远端 pane，行为不受影响）。

- [ ] **Step 5: i18n 键**

在三个 locale 文件的 CUA 权限文案区（`settings.computerUse.*` 与现有 `cua.permission` key 相邻处）加同一 key：

- `zh-CN.ts`：`"cua.permission.remoteOnboardingDescription": "Mode 需要在局域网对端设备上获得以下权限才能继续电脑控制：{permissions}。将在对端设备上打开权限引导，请在对端设备完成授权。"`
- `en-US.ts`：`"cua.permission.remoteOnboardingDescription": "Mode needs the following permissions on the LAN peer device to continue computer control: {permissions}. The permission onboarding will open on the peer device; complete the grant there."`
- `fa.ts`：同 en-US 文本（fa 对技术文案的既有处理以邻近 key 为准；先看邻近 CUA key 是否为英文原样，是则照做）。

占位符插值方式对齐该组件现有 `intl.formatMessage` 用法（读 Step 1 时确认；若现有 confirm 用数组拼接 description，就以同样方式传权限列表）。

- [ ] **Step 6: 跑测试与全量验证**

Run: `pnpm exec tsx --test packages/ui/test/*.test.ts && pnpm typecheck && pnpm lint && node scripts/check-language-policy.mjs`
Expected: 全绿。

- [ ] **Step 7: 提交**

```bash
git add packages/ui/src/cua-permission/CuaPermissionObservationAttachment.tsx packages/ui/src/cua-permission/cuaPermissionObservationSource.ts packages/ui/test/cuaPermissionObservationSource.test.ts packages/ui/src/i18n/locales/zh-CN.ts packages/ui/src/i18n/locales/en-US.ts packages/ui/src/i18n/locales/fa.ts
git commit -m "feat(cua): 权限观察附件多源订阅，对端观察走对端服务与引导"
```

（若 Step 4 未拆新文件，从 add 列表移除 `cuaPermissionObservationSource.ts`。）

---

### Task 9: 全量验证、spec 验收与注释同步

**Files:**
- Modify: `docs/specs/lan-remote-computer-use.md`（验收勾选）
- Modify: `packages/server/src/remote/modeAgentOfficialPluginAssets.ts:37-39`、`scripts/prepare-prebuilds.mjs:127-129`（注释口径）

**Interfaces:** 无新代码。

- [ ] **Step 1: 全量验证**

Run（逐条执行，如实记录输出）：

```bash
pnpm typecheck
pnpm lint
pnpm fmt:check
pnpm architecture:check --changed
node scripts/check-language-policy.mjs
pnpm --filter @mode/shared test
pnpm --filter @mode/services test
pnpm --filter @mode/server test
node --test "packages/desktop/tests/*.test.mjs"
node --test "packages/desktop/tests/*.test.mts"
pnpm exec tsx --test packages/ui/test/*.test.ts
pnpm --filter @mode/core test
```

Expected: 全绿。任何失败先修复再继续；环境受限/既有失败如实写进最终报告，不得跳过或美化。

- [ ] **Step 2: 注释口径同步**

两处「远端不承载 CUA」注释追加一句（不改合同本身，ssh/wsl/docker 部署仍不承载）：

```
// （2026-10-11）LAN 形态不经此合同：对端是完整 desktop 自己，CUA 经 LAN 能力协商启用，
// 见 docs/specs/lan-remote-computer-use.md。
```

- [ ] **Step 3: spec 验收勾选**

`docs/specs/lan-remote-computer-use.md`「验收场景」：勾选已由自动化覆盖的项（能力位缺键降级、入口/权限事件面单测、ssh/wsl/docker/手机回归）；**不勾**需两台实机的项（对端物理屏幕操作、对端弹 onboarding、握手端到端）——与 `lan-remote-attach.md` 的「待实机」体例一致，在该节追加一行「待实机清单」列出两台机器的验证步骤。

- [ ] **Step 4: 提交**

```bash
git add docs/specs/lan-remote-computer-use.md packages/server/src/remote/modeAgentOfficialPluginAssets.ts scripts/prepare-prebuilds.mjs
git commit -m "docs(specs): LAN CUA 验收勾选与远端资产注释口径同步"
```

---

## 计划自审记录（写完即查）

1. **Spec 覆盖**：能力协商（T1-T2）、对端 enablement（T3）、UI 四门（T4）、权限透传与归属（T5）、事件面（T6）、远程 onboarding（T7-T8）、降级语义（T1 缺键 / T4 `=== true` / T5 键出现判据 / T6 fail closed）、验收与注释（T9）。spec「接口变更 3 的归属判据」与 T5 一致（键出现=新对端）。
2. **占位符扫描**：无 TBD/TODO；T4/T6 中「读文件确认变量名/字段名」处均给了确切行号锚点与目标代码，属执行时对齐而非留白。
3. **类型一致性**：`RemoteWorkspaceSessionCapabilities { computerUse?: boolean }` 在 T2（store/bridge）与 T5（merge options 内联同形）一致；`openPermissionOnboarding(workspacePath?, workspaceIdentity?, options?)` 在 T7（定义）与 T8（调用）一致；`lanComputerUseCapable?: boolean` 在 T4 定义与两处消费一致。
