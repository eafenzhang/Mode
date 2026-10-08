import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  canonicalPluginId,
  isDefaultEnabledOfficialPluginId,
  isPublicStoreMarketplaceId,
  MODE_MCP_REQUEST_CONTEXT_META_KEY,
  MODE_MCP_REQUEST_CONTEXT_META_KEY_LEGACY,
  MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
  MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
  MODE_PROTOCOL_ACCEPTED_NAMES,
  MODE_PROTOCOL_NAME,
  MODE_PROTOCOL_VERSION,
  modeSessionStateSnapshotSchema,
  readModeMcpRequestContextMeta,
} from "@mode/shared";
import { resolveOfficialPluginCacheRoot } from "@mode/shared/node";

// S5c / S6：官方市场 id、协议名与 MCP _meta 键的改名兼容（见 docs/specs/p2-mode-naming.md）。
// 三条不变式：① 写入与判据用新值；② 存量旧值必须继续可认；③ 归一发生在读取边界、不改写用户数据。

test("官方市场 id：新名是 canonical，旧名继续归入公开分段", () => {
  assert.equal(MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID, "mode-plugins-official");
  assert.equal(MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID, "zcode-plugins-official");

  assert.equal(isPublicStoreMarketplaceId(MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID), true);
  assert.equal(isPublicStoreMarketplaceId(MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID), true);
  assert.equal(isPublicStoreMarketplaceId("claude-plugins-official"), false);
});

test("插件 id 读时归一：旧市场段映射到新市场段，其余原样", () => {
  assert.equal(
    canonicalPluginId("browser-use@zcode-plugins-official"),
    "browser-use@mode-plugins-official",
  );
  // 已归一的不再变动（幂等），非官方市场与非法形态原样返回。
  assert.equal(
    canonicalPluginId("browser-use@mode-plugins-official"),
    "browser-use@mode-plugins-official",
  );
  assert.equal(canonicalPluginId("foo@claude-plugins-official"), "foo@claude-plugins-official");
  assert.equal(canonicalPluginId("not-a-plugin-id"), "not-a-plugin-id");
  assert.equal(canonicalPluginId("zcode-plugins-official"), "zcode-plugins-official");

  // 默认启用判据两种写法都命中；computer-use 仍不在默认启用集合内。
  assert.equal(isDefaultEnabledOfficialPluginId("browser-use@mode-plugins-official"), true);
  assert.equal(isDefaultEnabledOfficialPluginId("browser-use@zcode-plugins-official"), true);
  assert.equal(isDefaultEnabledOfficialPluginId("computer-use@zcode-plugins-official"), false);
});

test("官方插件缓存根：新目录优先，新目录不存在时沿用旧目录", async () => {
  const storageRoot = await mkdtemp(join(tmpdir(), "mode-plugin-cache-"));
  try {
    // 两边都不存在：返回新目录（新装插件落到新根）。
    assert.equal(
      resolveOfficialPluginCacheRoot(storageRoot),
      join(storageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID),
    );

    // 只有旧目录（存量安装）：沿用旧目录，已装插件文件原地可用。
    await mkdir(join(storageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID), {
      recursive: true,
    });
    assert.equal(
      resolveOfficialPluginCacheRoot(storageRoot),
      join(storageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID),
    );

    // 新目录出现（seed 之后）：切到新目录，不再读旧快照。
    await mkdir(join(storageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID), {
      recursive: true,
    });
    assert.equal(
      resolveOfficialPluginCacheRoot(storageRoot),
      join(storageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID),
    );
  } finally {
    await rm(storageRoot, { force: true, recursive: true });
  }
});

test("协议名握手：对外声明新名，校验同时接受改名前的旧名", () => {
  assert.equal(MODE_PROTOCOL_NAME, "Mode Protocol");
  assert.deepEqual([...MODE_PROTOCOL_ACCEPTED_NAMES], ["Mode Protocol", "ZCode Protocol"]);

  // 用最小对象触发协议段校验：只要错误里没有 protocol.name，就说明该名字被接受。
  const nameIssuesFor = (name: string) => {
    const result = modeSessionStateSnapshotSchema.safeParse({
      protocol: { name, version: MODE_PROTOCOL_VERSION },
    });
    assert.equal(result.success, false, "缺字段的最小对象不应整体通过校验");
    return result.error.issues.filter((issue) => issue.path.join(".") === "protocol.name");
  };
  assert.equal(nameIssuesFor(MODE_PROTOCOL_NAME).length, 0, "新协议名必须被接受");
  assert.equal(nameIssuesFor("ZCode Protocol").length, 0, "远端旧 agent 的旧协议名必须被接受");
  assert.equal(nameIssuesFor("Mode Protocol v2").length, 1, "未知协议名仍要拒绝");
});

test("MCP _meta 请求上下文：新键优先，改名前的旧键兜底", () => {
  const context = { session_id: "s-1" };
  assert.deepEqual(readModeMcpRequestContextMeta({ [MODE_MCP_REQUEST_CONTEXT_META_KEY]: context }), context);
  assert.deepEqual(
    readModeMcpRequestContextMeta({ [MODE_MCP_REQUEST_CONTEXT_META_KEY_LEGACY]: context }),
    context,
  );
  // 两个键同时出现时以新键为准（不合并、不猜测）。
  assert.deepEqual(
    readModeMcpRequestContextMeta({
      [MODE_MCP_REQUEST_CONTEXT_META_KEY]: context,
      [MODE_MCP_REQUEST_CONTEXT_META_KEY_LEGACY]: { session_id: "stale" },
    }),
    context,
  );
  assert.equal(readModeMcpRequestContextMeta(undefined), undefined);
  assert.equal(readModeMcpRequestContextMeta({}), undefined);
});
