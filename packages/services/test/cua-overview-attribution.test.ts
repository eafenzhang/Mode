import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 自研插件署名（docs/specs/first-party-plugin-attribution.md 验收 1 的 overview 面）：
// 官方平台服务已下线（isOfficialServiceEnabled 恒 false → mode-plugins-official 不进
// DEFAULT 市场、known 记录缺失），getModePluginsOverview 读不到本地 canonical manifest，
// 随包插件的 listing 会落到官方目录快照兜底——快照里是上游 Z.ai。有 definitions 的包
// 必须改用 definitions 的 MODE_AUTHOR listing，快照只喂本分支没有本地包的条目。
const bootstrapPlugins = await import("../../../apps/mode-cli/packages/bootstrap/src/plugins.js");
const adaptersConfig = await import("../../../apps/mode-cli/packages/adapters/src/config/index.js");

async function makeOverviewFixture() {
  const root = await mkdtemp(join(tmpdir(), "cua-overview-"));
  const storageRoot = join(root, "plugins");
  const workingDirectory = join(root, "workspace");
  const configResult = adaptersConfig.createConfig({
    workingDirectory,
    skipUserConfig: true,
  });
  const overview = bootstrapPlugins.getModePluginsOverview({
    configResult,
    pluginStorageRoot: storageRoot,
    workingDirectory,
    env: {},
  });
  return overview;
}

test("overview 的 computer-use 条目署名 Mode，不回流快照 Z.ai", async () => {
  const overview = await makeOverviewFixture();
  const entry = overview.availablePlugins.find(
    (plugin) => plugin.id === "computer-use@mode-plugins-official",
  );
  assert.ok(entry, "availablePlugins 应包含 computer-use");
  assert.equal(
    entry.listing?.author,
    "Mode",
    "开发者必须来自 definitions 的 MODE_AUTHOR，而不是官方目录快照",
  );
  assert.equal(
    entry.listing?.authorUrl,
    undefined,
    "不携带上游站点链接（宁可缺省也不指向上游域名）",
  );
  assert.equal(entry.listing?.displayNameI18n?.["zh-CN"], "电脑控制");
});

test("官方快照只喂没有本地包的条目：第三方条目保持原样", async () => {
  const overview = await makeOverviewFixture();
  // cloudbase-skills 不在 OFFICIAL_PLUGIN_DEFINITIONS 里，必须继续来自官方快照且署名不变。
  const thirdParty = overview.availablePlugins.find(
    (plugin) => plugin.id === "cloudbase-skills@mode-plugins-official",
  );
  assert.ok(thirdParty, "快照中的第三方条目不应被过滤");
  assert.equal(thirdParty.listing?.author, "TencentCloudBase");
});
