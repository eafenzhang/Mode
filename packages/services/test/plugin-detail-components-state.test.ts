import assert from "node:assert/strict";
import test from "node:test";
import { resolvePluginDetailComponentsState } from "../../../packages/ui/src/settings/pluginDetailComponentsState";

// 需求：详情页对「确定拿不到安装包」的插件不再显示可重试的「组件清单加载失败」。
// 两类条目（未随包 = 本地没有包；源已下线 = 下载源在不再连接的官方平台）describe 必然
// 失败，呈现成失败态会让用户反复点重试。钉住：
//   1. 两类标记各自单独成态（unbundled / sourceUnavailable），与 describe 状态无关；
//   2. 已安装（有运行时信息）→ ready，仍按权威枚举渲染，不被标记影响；
//   3. 其余候选保留原有语义：未请求/请求中 = loading，失败 = error，成功 = ready。

test("未随包提供且无运行时信息：无论 describe 状态都固定为 unbundled", () => {
  for (const describeStatus of [undefined, "loading", "loaded", "error"] as const) {
    assert.equal(
      resolvePluginDetailComponentsState({
        bundledUnavailable: true,
        hasRuntimeInfo: false,
        describeStatus,
      }),
      "unbundled",
      `describeStatus=${String(describeStatus)}`,
    );
  }
});

test("源已下线且无运行时信息：无论 describe 状态都固定为 sourceUnavailable", () => {
  for (const describeStatus of [undefined, "loading", "loaded", "error"] as const) {
    assert.equal(
      resolvePluginDetailComponentsState({
        sourceUnavailable: true,
        hasRuntimeInfo: false,
        describeStatus,
      }),
      "sourceUnavailable",
      `describeStatus=${String(describeStatus)}`,
    );
  }
});

test("标记不覆盖已安装条目：有运行时信息仍走权威枚举", () => {
  assert.equal(
    resolvePluginDetailComponentsState({
      bundledUnavailable: true,
      sourceUnavailable: true,
      hasRuntimeInfo: true,
      describeStatus: undefined,
    }),
    "ready",
  );
});

test("普通候选：未请求或请求中为 loading，失败为 error，成功为 ready", () => {
  const base = { hasRuntimeInfo: false };
  assert.equal(resolvePluginDetailComponentsState({ ...base }), "loading");
  assert.equal(
    resolvePluginDetailComponentsState({ ...base, describeStatus: "loading" }),
    "loading",
  );
  assert.equal(resolvePluginDetailComponentsState({ ...base, describeStatus: "error" }), "error");
  assert.equal(resolvePluginDetailComponentsState({ ...base, describeStatus: "loaded" }), "ready");
});

test("标记只对未安装条目生效：有运行时信息时两类标记都不改写 ready", () => {
  assert.notEqual(
    resolvePluginDetailComponentsState({
      bundledUnavailable: true,
      hasRuntimeInfo: true,
    }),
    "unbundled",
  );
  assert.equal(
    resolvePluginDetailComponentsState({
      sourceUnavailable: true,
      hasRuntimeInfo: true,
    }),
    "ready",
  );
  assert.equal(
    resolvePluginDetailComponentsState({ bundledUnavailable: false, hasRuntimeInfo: false }),
    "loading",
    "未打标的普通候选不受影响",
  );
});
