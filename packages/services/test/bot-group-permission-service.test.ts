import assert from "node:assert/strict";
import test, { after } from "node:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// 群授权服务端操作（A+ 群规则 4/5）：pending 由入站发现，approve 后变 approved；
// reject/remove 删除授权并清空该群历史。测试走真实 services 图与落盘文件，
// 覆盖 mutateConfig 的持久化结果，而不是只测内存返回值。

const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-group-permission-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
process.env.MODE_DESKTOP_HOME_DIR = dataDir;

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
const shared = await import("@mode/shared");
const node = await import("@mode/services/node");
const services = await import("@mode/services");

const BOT_ID = "bot-1";

function buildBotConfig() {
  return shared.botsConfigFileSchema.parse({
    version: 3,
    bots: [
      {
        id: BOT_ID,
        name: BOT_ID,
        // 关闭运行时连接：本用例只验证配置与群历史，不需要真实通道。
        enabled: false,
        provider: "wecom",
        allowedWorkspaces: ["*"],
        allowedCommands: {
          status: true,
          new: true,
          workspace: true,
          model: true,
          mode: true,
          thoughtLevel: true,
          reply: true,
        },
        currentOptions: {},
        replyMode: "streaming_card",
        groupChat: {
          activation: "mention",
          permissions: [
            {
              groupId: "group-A",
              groupName: "Group A",
              platform: "wecom",
              status: "pending",
              discoveredAt: 1,
            },
            {
              groupId: "group-B",
              groupName: "Group B",
              platform: "wecom",
              status: "pending",
              discoveredAt: 2,
            },
          ],
        },
      },
    ],
  });
}

let hostServices: Awaited<ReturnType<typeof node.createLocalServices>> | null = null;

after(async () => {
  if (hostServices) {
    await node.disposeServiceResourcesAndWait(hostServices).catch(() => undefined);
  }
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {
    // best-effort；Windows 句柄晚释放不应把测试判红。
  }
});

function readPersistedConfig() {
  const raw = JSON.parse(
    readFileSync(join(node.getAppConfigDir(), "bot-config.v3.json"), "utf8"),
  ) as { bots: Array<{ groupChat?: { permissions?: Array<{ groupId: string; status: string }> } }> };
  return raw.bots.find(() => true);
}

test("approve/reject/remove：配置持久化、缺失群报错、拒绝/移除清空群历史", async () => {
  const configDir = node.getAppConfigDir();
  mkdirSync(configDir, { recursive: true });
  writeFileSync(
    join(configDir, "bot-config.v3.json"),
    `${JSON.stringify(buildBotConfig(), null, 2)}\n`,
  );
  writeFileSync(
    join(configDir, "bot-group-history.v1.json"),
    `${JSON.stringify({
      version: 1,
      bots: {
        [BOT_ID]: {
          "group-A": [{ text: "a-before-approve", at: 1 }],
          "group-B": [{ text: "b-before-reject", at: 2 }],
        },
      },
    })}\n`,
  );

  hostServices = await node.createLocalServices({
    modeBuiltinProviderConfigFilePath: join(repoRoot, "config/provider/mode-builtin.json"),
  });
  const botsService = hostServices.get(services.IBotsService);

  // 未发现的群不能直接批准。
  await assert.rejects(
    () => botsService.approveBotGroup({ botId: BOT_ID, groupId: "missing-group" }),
    /Group not found/,
  );

  const approved = await botsService.approveBotGroup({ botId: BOT_ID, groupId: "group-A" });
  assert.equal(
    approved.groupChat?.permissions?.find((permission) => permission.groupId === "group-A")?.status,
    "approved",
  );
  assert.equal(
    approved.groupChat?.permissions?.find((permission) => permission.groupId === "group-B")?.status,
    "pending",
    "批准一个群不能影响其他待审批群",
  );
  assert.equal(
    readPersistedConfig().groupChat?.permissions?.find(
      (permission) => permission.groupId === "group-A",
    )?.status,
    "approved",
    "批准结果必须落盘",
  );

  const rejected = await botsService.rejectBotGroup({ botId: BOT_ID, groupId: "group-B" });
  assert.equal(
    rejected.groupChat?.permissions?.some((permission) => permission.groupId === "group-B"),
    false,
    "拒绝后授权记录必须删除",
  );

  const removed = await botsService.removeBotGroup({ botId: BOT_ID, groupId: "group-A" });
  assert.equal(removed.groupChat?.permissions?.length, 0, "移除后不应残留授权");

  // 群历史落盘有 2s 去抖；等待一次完整窗口后检查文件。
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  const history = JSON.parse(
    readFileSync(join(configDir, "bot-group-history.v1.json"), "utf8"),
  ) as { bots: Record<string, Record<string, unknown>> };
  assert.equal(history.bots[BOT_ID]?.["group-A"], undefined, "拒绝/移除必须清空群 A 历史");
  assert.equal(history.bots[BOT_ID]?.["group-B"], undefined, "拒绝必须清空群 B 历史");
});
