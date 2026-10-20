/**
 * LR-0：频道与注册边界的静态契约。
 *
 * 1. 频道名是跨进程字符串：`ServiceChannels`、descriptor、Host 注册必须同值；
 * 2. `lan-access` 只能注册在 **local-only** 集合上（`host-standalone/serviceAccessor.ts`），
 *    否则 LAN 对端会通过 `createHttpServer` 拿到管理面（`pairPeer` 返回长期令牌）。
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

import { ILanAccessService, IRemoteDesktopService, IRemoteService } from "@mode/services";
import { ServiceChannels } from "@mode/shared";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const read = (relativePath: string): string =>
  readFileSync(resolve(repoRoot, relativePath), "utf8");

test("lan-access 频道名在 descriptor 与 ServiceChannels 中一致", () => {
  assert.equal(ServiceChannels.LanAccess, "lan-access");
  assert.equal(ILanAccessService.channelName, ServiceChannels.LanAccess);
  assert.equal(ServiceChannels.RemoteDesktop, "remote-desktop");
  assert.equal(IRemoteDesktopService.channelName, ServiceChannels.RemoteDesktop);
  assert.equal(ServiceChannels.Remote, "remote");
  assert.equal(IRemoteService.channelName, ServiceChannels.Remote);
});

test("管理面注册在 local-only 集合上，普通服务集合先暴露", () => {
  const source = read("packages/desktop/src/host-standalone/serviceAccessor.ts");
  // 创建独立集合、在它上面注册管理频道。
  assert.match(source, /new ServiceCollection\(\)/u);
  assert.match(source, /localOnly\.register\(ILanAccessService/u);
  assert.match(source, /localOnly\.register\(IRemoteDesktopService/u);
  assert.match(source, /localOnly\.register\(IRemoteService/u);
  // 普通集合不能注册管理频道（否则 `createHttpServer` 会把它暴露给 LAN 对端）。
  assert.doesNotMatch(source, /services\.register\(ILanAccessService/u);
  assert.doesNotMatch(source, /services\.register\(IRemoteDesktopService/u);
  assert.doesNotMatch(source, /services\.register\(IRemoteService/u);
  // 两个集合都注册到窗口 ChannelServer；普通集合必须带窗口级可信连接作用域。
  assert.match(
    source,
    /services\.exposeOnChannelServer\(guardedServer, rendererScope\.overrides\)/u,
  );
  assert.match(source, /localOnly\.exposeOnChannelServer\(guardedServer\)/u);
  // 销毁顺序：先停监听，再收被控会话与远程工作区，然后释放连接作用域，最后释放服务资源。
  const lanDispose = source.indexOf("await lanAccess.dispose()");
  const rdDispose = source.indexOf("await remoteDesktop.dispose()");
  const remoteDispose = source.indexOf("await remote.disposeHost()");
  const scopeDispose = source.indexOf("await rendererScope.dispose()");
  const servicesDispose = source.indexOf("await disposeServiceResourcesAndWait(services)");
  assert.ok(
    lanDispose >= 0 &&
      rdDispose > lanDispose &&
      remoteDispose > rdDispose &&
      scopeDispose > remoteDispose &&
      servicesDispose > scopeDispose,
    "dispose 顺序必须是 LAN 监听 → 被控会话 → 远程工作区 → 连接作用域 → 服务资源",
  );
});
