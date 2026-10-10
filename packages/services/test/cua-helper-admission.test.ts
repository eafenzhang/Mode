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
  // ssh identity 必须含 username 段才可解析（remote-workspace-identity.ts 格式契约
  // remote:ssh:<host>:<port>:<username>:<posixPath>），此处用可解析样例真正走 kind 排除分支。
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:ssh:host:22:alice:/tmp/ws"), true);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:wsl:Ubuntu:/tmp/ws"), true);
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:docker:abc:/tmp/ws"), true);
  // 段数不合规范、无法解析的非空串按历史「有 identity 即挡」fail closed。
  assert.equal(isCuaExcludedRemoteWorkspaceIdentity("remote:ssh:host:22:/tmp/ws"), true);
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
        "remote:ssh:host:22:alice:/tmp/ws",
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
