/**
 * M3 Task 2 · Step 2：**真服务面**（DR-T1-1 的 stub 换成 `packages/services` 的真实装配）。
 *
 * 这里守住五件事，每一件都对应一条「空壳时代看不出来、换上真服务才会暴露」的失败：
 *
 * 1. **真命令真返回**：`system.info()` / `setting.get()` 走端口拿到真实值。Task 1 的 stub 对每个
 *    命令都 `not_implemented`，所以「闸门放行」当时并不代表任何业务面可用；本用例把
 *    「至少一个真实业务面可用」变成断言（阶段验收第 1 条）。
 * 2. **`listen` 抛错不能杀死 Host**（Task 1 的 C1，spec §已知危险）：真服务由 `ProxyChannel.fromService`
 *    包装，它的 `listen` 对未知事件名**直接抛** `Event not found`（`packages/rpc/src/proxy-channel.ts`），
 *    而 `ChannelServer.onEventListen` 没有 try/catch —— 一次坏订阅就能带走整个 Host。守卫必须
 *    是**一次性的、装在派发边界上**的（`serviceAccessor.ts` 的频道守卫 + 入口的派发守卫），
 *    不是逐个服务打补丁。
 * 3. **服务写 stdout 不能污染帧流**：真服务在进程内 `console.log`（实测：`cua-pip-session` 的
 *    wiring 诊断就走 stdout），而 Host 的 stdout **只用于帧** —— 一次日志就把长度前缀流冲散。
 *    守卫把非帧写入转到 stderr 并计数。
 * 4. **数据根口径（Task 1 的 I2）**：真服务会**写盘**，所以这一条在本任务第一次有了实际后果：
 *    写入必须落在 `{base}/.mode`，**不能**落在 `{base}/.mode/.mode`。
 * 5. **闸门由真启动序列驱动**：`ready` 由「真存储 → 真服务装配」推进出来，且只在全部频道注册
 *    之后发布（`deferInit`）——即壳的「第一帧」判据就是「服务面齐备」。
 */
import { existsSync, mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import {
  callWithTimeout,
  connectProductionHost,
  describeHost,
  disposeHost,
  removeSandbox,
} from "./fixtures/productionHost.mts";

const delayMs = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/** 一个隔离沙箱：真实服务会在 `data` 下写 `.mode` 树。 */
function sandbox(prefix: string): { root: string; home: string; dataBaseDir: string } {
  const root = mkdtempSync(join(tmpdir(), prefix));
  return { root, home: join(root, "home"), dataBaseDir: join(root, "data") };
}

test("真实服务面：真实业务命令经端口返回真值（不再是 stub 的 not_implemented）", async () => {
  const box = sandbox("mode-host-real-");
  const connection = await connectProductionHost({ home: box.home, dataBaseDir: box.dataBaseDir });
  try {
    const describe = await describeHost(connection);
    assert.equal(
      describe.serviceSurface?.kind,
      "services",
      `Host 必须报告自己装的是真服务面：${JSON.stringify(describe.serviceSurface)}`,
    );

    // 真命令 ①：system.info() 报本机事实，且 homedir 来自**隔离的家目录**（服务确实读我们给的环境）。
    const info = await callWithTimeout(
      connection.client.getChannel("system").call<{ homedir: string; platform: string }>("info"),
      "system.info",
    );
    assert.equal(info.platform, process.platform, "system.info() 报的是本进程平台");
    assert.equal(
      info.homedir.replace(/\\/g, "/").toLowerCase(),
      box.home.replace(/\\/g, "/").toLowerCase(),
      "system.info().homedir 必须等于隔离的家目录（数据隔离在服务层也生效）",
    );

    // 真命令 ②：setting.get() 返回真实设置对象（stub 只会 not_implemented）。
    const settings = await callWithTimeout(
      connection.client.getChannel("setting").call<Record<string, unknown>>("get"),
      "setting.get",
    );
    assert.ok(settings && typeof settings === "object", "setting.get() 必须返回设置对象");
    assert.ok("locale" in settings, `settings 应是真设置：${Object.keys(settings).join(",")}`);

    // stub 时代最容易被读成「成功」的假象源：未知命令必须是 ProxyChannel 的 `Method not found`，
    // 而不是 stub 的 `not_implemented`（那正是「还没实现」被读成「成功但没数据」的来源）。
    await assert.rejects(
      callWithTimeout(
        connection.client.getChannel("setting").call("__m3_task2_probe__"),
        "setting.__m3_task2_probe__",
      ),
      (error: unknown) => {
        const message = error instanceof Error ? error.message : String(error);
        assert.match(message, /Method not found/, `真实服务面应报 Method not found，实际：${message}`);
        assert.doesNotMatch(message, /not_implemented/, "stub 的标记必须消失（服务面已换真）");
        return true;
      },
    );
    assert.ok(
      (describe.serviceChannels?.length ?? 0) >= 40,
      `真实装配必须暴露全部服务频道，实际 ${describe.serviceChannels?.length}`,
    );
  } finally {
    await disposeHost(connection);
    await removeSandbox(box.root);
  }
});

test("真实服务的 listen 守卫：订阅一个不存在的事件不会杀死 Host，也不给「订阅成功」的假象", async () => {
  // 这条守的是一条**能杀掉 Host** 的真路径：`ProxyChannel.fromService` 的 `listen` 对未知事件名抛
  // `Event not found`，而 `ChannelServer.onEventListen` 没有 try/catch（spec §已知危险）。真服务面里
  // 事件名由各服务实现决定，任何一处名字对不上（渲染层按 host 无关的方式订阅）都会走到这里。
  const box = sandbox("mode-host-listen-");
  const connection = await connectProductionHost({ home: box.home, dataBaseDir: box.dataBaseDir });
  try {
    // `setting` 的实现只有方法、没有任何 `on*` 事件 → 任何事件名都必然走「未知事件」分支。
    const received: unknown[] = [];
    const subscription = connection.client
      .getChannel("setting")
      .listen<unknown>("onDidChange")((value: unknown) => received.push(value));
    await delayMs(500);

    // ① 订阅没有把 Host 带走（本用例存在的首要理由）。
    assert.equal(
      connection.child.exitCode,
      null,
      `坏订阅之后 Host 必须还活着；stderr:\n${connection.stderr()}`,
    );
    // ② 同一条传输上的后续 RPC 仍然可用。
    const again = await describeHost(connection);
    assert.equal(again.name, "mode-host-sidecar");
    // ③ 不假装有事件：一个都没投。
    assert.deepEqual(received, [], "守卫不投假事件");
    // ④ 也不静默：stderr 留痕 + `host.describe` 上可观察。
    assert.match(connection.stderr(), /Event not found|listen_guard/, "拒绝必须留痕（stderr）");
    assert.ok(
      (again.serviceSurface?.guardedSubscriptions ?? 0) >= 1,
      `host.describe 应能观察到 guardedSubscriptions，实际 ${JSON.stringify(again.serviceSurface)}`,
    );
    subscription.dispose();

    // ⑤ 诊断频道（`host`）也过守卫。Task 1 复评留了一条 deferred minor：`host` channel 自己的
    //    `listen` 就是抛的（`unknown command` 那一族），与「任何服务的 `listen` 抛错都会杀死
    //    Host」这条刚写进 spec 的规则直接矛盾。守卫装在 `registerChannel` 这个唯一入口上，
    //    所以它一并被覆盖——这里把「不再矛盾」钉住（否则那句话只是口号）。
    const hostReceived: unknown[] = [];
    const hostSubscription = connection.client
      .getChannel("host")
      .listen<unknown>("onAnything")((value: unknown) => hostReceived.push(value));
    await delayMs(300);
    assert.equal(connection.child.exitCode, null, "订阅诊断频道事件同样不得杀死 Host");
    assert.deepEqual(hostReceived, [], "诊断频道也没有事件可投");
    assert.match(connection.stderr(), /listen_guard host\.onAnything/, "诊断频道的拒绝同样留痕");
    hostSubscription.dispose();
  } finally {
    await disposeHost(connection);
    await removeSandbox(box.root);
  }
});

test("stdout 守卫：真实服务写 stdout 不会污染帧流，字节被转到 stderr 并计数", async () => {
  // 帧流是 `[4 字节大端长度][payload]`：任何一行服务日志混进 stdout 都会让读方对不齐长度前缀，
  // 于是「RPC 全挂」——而现象是页面侧的协议层静默丢帧，最难查。
  const box = sandbox("mode-host-stdout-");
  const connection = await connectProductionHost({ home: box.home, dataBaseDir: box.dataBaseDir });
  try {
    const describe = await describeHost(connection);
    const surface = describe.serviceSurface;
    assert.ok(surface, "host.describe 必须报告服务面状态");
    // 真服务在启动期确实会写 stdout（实测：cua-pip-session 的 wiring 诊断）。若服务日后不再写，
    // 这条会红——这是想要的「响一声」，不是脆弱：那时要么删掉这条断言，要么换一条真实的 stdout 写入方。
    assert.ok(
      surface.stdoutDiverted >= 1,
      `启动期必须有被拦下的 stdout 写入；实际 ${surface.stdoutDiverted}，samples=${JSON.stringify(surface.stdoutSamples)}`,
    );
    assert.ok(
      surface.stdoutSamples.some((sample) => sample.includes("cua-pip-session")),
      `拦下的样本里应能看到服务日志：${JSON.stringify(surface.stdoutSamples)}`,
    );
    // 关键：帧流完好——一次真 RPC 还能走通。
    const info = await callWithTimeout(
      connection.client.getChannel("system").call<{ platform: string }>("info"),
      "system.info",
    );
    assert.equal(info.platform, process.platform);
  } finally {
    await disposeHost(connection);
    await removeSandbox(box.root);
  }
});

test("真启动序列：ready 由真存储 + 真服务装配推进，且只在全部服务频道注册之后发布", async () => {
  // 这条守的是「闸门由**真实** startup 状态驱动」这句话（阶段验收第 2 条）。壳的就绪判据是
  // 「Host 的第一帧」（`ChannelServer` 的 `Initialize`），而入口用了 `deferInit = true`：
  // `server.ready()` 在**全部频道注册之后**才调。所以「闸门放行」与「服务面齐备」在时序上
  // 是同一件事，而不是「合成了一个 ready」——这正是 Task 2 要换掉 Task 1 那个合成值的地方。
  const box = sandbox("mode-host-startup-");
  const connection = await connectProductionHost({ home: box.home, dataBaseDir: box.dataBaseDir });
  try {
    const describe = await describeHost(connection);

    // ① 终态是 ready，且没有失败字段。
    assert.equal(describe.startup?.phase, "ready", `真启动必须走到 ready；${JSON.stringify(describe.startup)}`);
    assert.equal(describe.startup?.errorCode, undefined, "没失败就不得出现 errorCode");
    assert.equal(describe.startup?.failedPhase, undefined);

    // ② 应用阶段的历史里必须看到真存储与真服务两步，且顺序正确。
    const history = describe.startup?.history ?? [];
    const indexOf = (phase: string): number =>
      history.findLastIndex((entry) => entry.endsWith(`:${phase}`));
    const storage = indexOf("preparing_host_storage");
    const services = indexOf("starting_services");
    const ready = indexOf("ready");
    assert.ok(storage >= 0, `历史里应有真存储那一步；实际 ${history.join(",")}`);
    assert.ok(services > storage, `服务装配必须晚于存储；实际 ${history.join(",")}`);
    assert.equal(ready, history.length - 1, `ready 必须是最后一次发布；实际 ${history.join(",")}`);

    // ③ 真存储确实建了库（不是「声称准备了」）：任务索引库出现在数据根下。
    assert.equal(
      existsSync(join(box.dataBaseDir, ".mode", "v2")),
      true,
      `真存储必须在 {base}/.mode 下建出库；实际 ${box.dataBaseDir} 下：` +
        (existsSync(box.dataBaseDir) ? readdirSync(box.dataBaseDir).join(",") : "(不存在)"),
    );

    // ④ 与就绪同代的另一件事：`describe` 能答就说明 `Initialize` 已发过，而它是全部频道注册
    //    之后才发的 —— 所以这里的频道数必须是**全量**（不是「先就绪后补频道」）。
    assert.ok(
      (describe.serviceChannels?.length ?? 0) >= 40,
      `就绪时服务频道应已全量注册，实际 ${describe.serviceChannels?.length}`,
    );
    assert.equal(describe.serviceSurface?.kind, "services");
  } finally {
    await disposeHost(connection);
    await removeSandbox(box.root);
  }
});

test("数据根口径：真实服务的写入落在 {base}/.mode，不出现 {base}/.mode/.mode", async () => {
  const box = sandbox("mode-host-dataroot-");
  const connection = await connectProductionHost({ home: box.home, dataBaseDir: box.dataBaseDir });
  try {
    // 触发一次真读（真服务会初始化自己的存储/设置文件）。
    await callWithTimeout(
      connection.client.getChannel("setting").call<Record<string, unknown>>("get"),
      "setting.get",
    );
    await delayMs(300);

    const dataRoot = join(box.dataBaseDir, ".mode");
    assert.equal(
      existsSync(dataRoot),
      true,
      `真服务必须把数据写进 {base}/.mode；实际 ${box.dataBaseDir} 下：` +
        (existsSync(box.dataBaseDir) ? readdirSync(box.dataBaseDir).join(",") : "(不存在)"),
    );
    assert.equal(
      existsSync(join(dataRoot, ".mode")),
      false,
      "数据根绝不能是 {base}/.mode/.mode（那把基目录当根用的口径错误）",
    );
    const describe = await describeHost(connection);
    assert.equal(describe.dataBaseDir, box.dataBaseDir, "MODE_DATA_BASE_DIR 仍是**基**目录");
    assert.equal(describe.dataRoot, dataRoot, "报告的数据根 = {base}/.mode");
  } finally {
    await disposeHost(connection);
    await removeSandbox(box.root);
  }
});
