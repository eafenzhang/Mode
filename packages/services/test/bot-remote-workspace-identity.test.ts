import assert from "node:assert/strict";
import test, { after } from "node:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// 远程工作区 IM 机器人绑定的 identity 口径回归（docs/specs/im-bot-remote-workspace-binding.md）：
// 1) settings 读出必须为缺 identity 的远端条目补齐 buildRemoteWorkspaceIdentity 派生值；
// 2) 存量 path-only 绑定表/allowedWorkspaces 在首次绑定读取时升级到 identity key，
//    否则会话绑定资格按 identity 查询永远看不到它们（「该机器人不属于当前工作区」），
//    本地工作区绑定保持 path key 不被误升级；
// 3) 远程 workspace scope 不得自建第二个 BotsService（实例内存态会分裂），必须复用 Local Host 实例。
// 数据目录隔离：MODE_DATA_BASE_DIR 必须在 @mode/services 首次 import 前生效（paths 模块加载时读取）。
// MODE_DESKTOP_HOME_DIR 同理——设置文件走 resolveUserHomeDir()（HOME/USERPROFILE），
// 不认 MODE_DATA_BASE_DIR；漏掉它会把测试数据写进真实 ~/.mode/v2/setting.json。
const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-identity-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
process.env.MODE_DESKTOP_HOME_DIR = dataDir;

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));

const shared = await import("@mode/shared");
const node = await import("@mode/services/node");
const services = await import("@mode/services");

const REMOTE_PATH = "/srv/app";
const REMOTE_TARGET = { kind: "lan" as const, host: "10.0.0.5", port: 45879 };
const REMOTE_IDENTITY = shared.buildRemoteWorkspaceIdentity(REMOTE_PATH, REMOTE_TARGET);
const LOCAL_PATH = "C:\\work\\local";

function buildBotEntry(id: string, allowedWorkspaces: string[]) {
  return {
    id,
    name: id,
    provider: "webhook" as const,
    enabled: true,
    providerUserId: `${id}-user`,
    allowedWorkspaces,
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
    replyMode: "assistant_changes" as const,
  };
}

let hostServices: Awaited<ReturnType<typeof node.createLocalServices>> | null = null;

after(async () => {
  if (hostServices) {
    await node.disposeServiceResourcesAndWait(hostServices).catch(() => undefined);
  }
  // Windows 上句柄晚释放会让 rmSync 抛错；清理失败不应把测试判红（临时目录由系统回收）。
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {
    // best-effort
  }
});

test("settings 读出为 path-only 远端条目补齐统一构造的 workspaceIdentity", async () => {
  const parsed = shared.appSettingsSchema.parse({
    lastWorkspaceSession: [
      {
        kind: "remote",
        workspacePath: REMOTE_PATH,
        target: REMOTE_TARGET,
        lastOpenedAt: 1,
        lastConnectionStatus: "connected",
      },
    ],
  });
  assert.equal(
    parsed.lastWorkspaceSession[0]?.workspaceIdentity,
    REMOTE_IDENTITY,
    "remote 条目缺 workspaceIdentity 时必须按 buildRemoteWorkspaceIdentity(path, target) 补齐",
  );
});

test("存量 path-only 绑定与授权升级到 identity key；本地工作区绑定保持不动", async () => {
  // 预置「identity 引入前」的落盘数据：绑定表与 allowedWorkspaces 都是 path-only。
  const configDir = node.getAppConfigDir();
  mkdirSync(configDir, { recursive: true });
  const config = shared.botsConfigFileSchema.parse({
    version: 3,
    bots: [buildBotEntry("legacy-bot", [REMOTE_PATH]), buildBotEntry("local-bot", [LOCAL_PATH])],
  });
  writeFileSync(join(configDir, "bot-config.v3.json"), `${JSON.stringify(config, null, 2)}\n`);
  const bindings = shared.botBindingsFileSchema.parse({
    version: shared.BOT_BINDINGS_FILE_VERSION,
    bindings: { [REMOTE_PATH]: ["legacy-bot"], [LOCAL_PATH]: ["local-bot"] },
  });
  writeFileSync(join(configDir, "bot-bindings.v3.json"), `${JSON.stringify(bindings, null, 2)}\n`);

  hostServices = await node.createLocalServices({
    modeBuiltinProviderConfigFilePath: join(repoRoot, "config/provider/mode-builtin.json"),
  });
  const botsService = hostServices.get(services.IBotsService);
  const settingService = hostServices.get(services.ISettingService);

  // 设置条目不带 identity（旧版本落盘形态）：读出必须补齐，绑定升级才有 identity key 可用。
  await settingService.update({
    lastWorkspaceSession: [
      {
        kind: "remote",
        workspacePath: REMOTE_PATH,
        target: REMOTE_TARGET,
        lastOpenedAt: 1,
        lastConnectionStatus: "connected",
      },
      { kind: "local", workspacePath: LOCAL_PATH },
    ],
  });
  const settings = await settingService.get();
  assert.equal(
    settings.lastWorkspaceSession.find((entry) => entry.kind === "remote")?.workspaceIdentity,
    REMOTE_IDENTITY,
    "settings 持久化读出后 remote 条目应带统一构造的 identity",
  );

  // 触发首次绑定读取（触发一次性升级）：远端绑定必须能在 identity key 下查到。
  const remoteBinding = await botsService.getWorkspaceBotBinding({
    workspacePath: REMOTE_PATH,
    workspaceIdentity: REMOTE_IDENTITY,
  });
  assert.deepEqual(remoteBinding.botIds, ["legacy-bot"]);

  const nextConfig = await botsService.getConfig();
  const legacyBot = nextConfig.bots.find((bot) => bot.id === "legacy-bot");
  assert.ok(legacyBot?.allowedWorkspaces.includes(REMOTE_IDENTITY), "授权应升级为 identity key");
  assert.ok(!legacyBot?.allowedWorkspaces.includes(REMOTE_PATH), "升级后不应再残留 path-only 授权");

  // 本地工作区没有 identity：path key 就是它的唯一 key，不能被误升级成远端 identity。
  const localBinding = await botsService.getWorkspaceBotBinding({ workspacePath: LOCAL_PATH });
  assert.deepEqual(localBinding.botIds, ["local-bot"]);
  const localBot = nextConfig.bots.find((bot) => bot.id === "local-bot");
  assert.deepEqual(localBot?.allowedWorkspaces, [LOCAL_PATH]);
});

test("远程 workspace scope 复用 Local Host BotsService，不自建第二个实例", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(
    new URL(
      "../../../packages/desktop/src/host/remoteWorkspaceServiceCollection.ts",
      import.meta.url,
    ),
    "utf8",
  );
  assert.ok(
    !source.includes("createBotsService"),
    "远程 scope 禁止 createBotsService 自建实例：绑定码/流订阅/心跳都在实例内存里，双实例必然分裂",
  );
  assert.ok(
    source.includes("getOptional(IBotsService)"),
    "远程 scope 必须复用 Local Host 注册的 IBotsService（含 remoteWorkspaceService 桥）",
  );
  assert.ok(
    source.includes("shieldSharedBotsServiceOwnership"),
    "复用实例必须屏蔽 disposeAll*：远程连接断开不能关停本机 bot 运行时",
  );
});
