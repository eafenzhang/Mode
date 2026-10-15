/**
 * 系统 RDP 主控面纯逻辑与生命周期夹具
 * （spec: docs/specs/remote-desktop-system-rdp.md 接口节「.rdp 模板」与「安全」）。
 *
 * 覆盖：
 * - `buildRdpFile`：选项矩阵、地址/用户名**注入拒绝**（`.rdp` 是 key:type:value 文本格式，
 *   值里含换行即可注入任意选项）、固定认证项、绝不写 password 字段；
 * - `createRdpLauncher`：注入 spawn 的启动/退出跟踪/断开/退出清理；
 * - `createRdpOptionsStore`：写读回环、损坏降级；
 * - `createSystemRdpProbe` + 解析函数：注册表取值解析、TTL 缓存、失败保留上次值。
 *
 * 失效模式（未实现时）：注入串被写进 `.rdp` / 会话退出后临时文件残留 / 缓存击穿做 IO。
 */
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 与其它桌面夹具同口径：先隔离数据目录再动态导入（防止传递依赖读宿主真实目录）。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rdp-"));

const { buildRdpFile, createRdpLauncher, isValidRdpAddress, isValidRdpUsername } = await import(
  "../../src/main/remoteDesktop/rdpLauncher.js"
);
const { createRdpOptionsStore } = await import("../../src/main/remoteDesktop/rdpOptions.js");
const {
  createSystemRdpProbe,
  isRdpHostEditionSupported,
  parseFDenyTSConnections,
  parseRegValue,
} = await import("../../src/host/systemRdpState.js");

// ────────────────────────── buildRdpFile ──────────────────────────

// 缺省档：窗口模式、剪贴板开、音频带到本机、端口 3389 不写后缀。
const defaults = buildRdpFile({ address: "10.0.0.5", options: {} });
assert.equal(defaults.ok, true, "合法 IPv4 必须生成成功");
const defaultText = defaults.ok ? defaults.content : "";
assert.match(defaultText, /^full address:s:10\.0\.0\.5\r\n/u, "默认端口不追加后缀");
assert.match(defaultText, /screen mode id:i:1/u, "缺省窗口模式");
assert.match(defaultText, /redirectclipboard:i:1/u, "剪贴板缺省开");
assert.match(defaultText, /audiomode:i:0/u, "音频缺省带到本机");
assert.match(defaultText, /enablecredsspsupport:i:1/u, "不降低认证等级");
assert.match(defaultText, /authentication level:i:2/u, "要求服务器身份验证");
assert.match(defaultText, /prompt for credentials:i:1/u, "凭据交给 Windows 收集");
assert.doesNotMatch(defaultText, /password 51:b:/u, "绝不写密码字段");
assert.doesNotMatch(defaultText, /(?<!\r)\n/u, "只允许 \\r\\n 行尾（不允许裸 \\n）");
assert.equal(defaultText.endsWith("\r\n"), true, "以 CRLF 结尾");

// 全选项 + 自定义端口 + 主机名。
const full = buildRdpFile({
  address: "desk-pc",
  options: {
    fullscreen: true,
    multimon: true,
    clipboard: false,
    audio: "remote",
    username: "CORP\\alice",
    port: 3390,
  },
});
assert.equal(full.ok, true, "合法主机名 + 自定义端口必须成功");
const fullText = full.ok ? full.content : "";
assert.match(fullText, /^full address:s:desk-pc:3390\r\n/u, "非默认端口才追加后缀");
assert.match(fullText, /username:s:CORP\\alice/u, "预填用户名");
assert.match(fullText, /screen mode id:i:2/u, "全屏");
assert.match(fullText, /use multimon:i:1/u, "多显示器");
assert.match(fullText, /redirectclipboard:i:0/u, "显式关闭剪贴板");
assert.match(fullText, /audiomode:i:1/u, "音频留在被控");

// ── 安全：注入拒绝（本夹具最重要的一组断言）──
const injectAddress = buildRdpFile({
  address: "10.0.0.5\r\nalternate shell:s:cmd.exe",
  options: {},
});
assert.equal(injectAddress.ok, false, "地址含 CRLF 必须被拒绝");
assert.equal(injectAddress.ok === false ? injectAddress.reason : "", "invalid_address");

const injectHost = buildRdpFile({ address: "10.0.0.5 evil", options: {} });
assert.equal(injectHost.ok, false, "地址含空格必须被拒绝");

const injectNewlineOnly = buildRdpFile({ address: "10.0.0.5\nfoo", options: {} });
assert.equal(injectNewlineOnly.ok, false, "地址含裸换行必须被拒绝");

const injectUser = buildRdpFile({
  address: "10.0.0.5",
  options: { username: "alice\r\nalternate shell:s:cmd.exe" },
});
assert.equal(injectUser.ok, false, "用户名含 CRLF 必须被拒绝");
assert.equal(injectUser.ok === false ? injectUser.reason : "", "invalid_username");

const injectUserTab = buildRdpFile({
  address: "10.0.0.5",
  options: { username: "alice\u0009x" },
});
assert.equal(injectUserTab.ok, false, "用户名含控制字符必须被拒绝");

// 地址形态白名单。
assert.equal(isValidRdpAddress("192.168.1.10"), true);
assert.equal(isValidRdpAddress("nas.local"), true);
assert.equal(isValidRdpAddress("a-b_c.example.com"), true);
assert.equal(isValidRdpAddress("256.1.1.1"), false, "IPv4 八位组越界");
assert.equal(isValidRdpAddress(""), false);
assert.equal(isValidRdpAddress("host:3389"), false, "地址与端口必须分开传（冒号非 IPv6 形态）");
assert.equal(isRdpHostEditionSupported("Professional").editionOk, true);
assert.equal(isRdpHostEditionSupported("Core").editionOk, false, "家庭版不可承载 RDP 主机");
assert.equal(isRdpHostEditionSupported("CoreSingleLanguage").editionOk, false);
assert.equal(isRdpHostEditionSupported(undefined).editionOk, true, "未知版本按放行（由 mstsc 兜底）");
assert.equal(isRdpHostEditionSupported(undefined).recognized, false);
assert.equal(isValidRdpUsername("CORP\\alice"), true);
assert.equal(isValidRdpUsername("a@b.com"), true);
assert.equal(isValidRdpUsername("   "), false);

// IPv6：含冒号，一律不追加端口后缀。
const v6 = buildRdpFile({ address: "fe80::1", options: { port: 3390 } });
assert.equal(v6.ok, true);
assert.match(v6.ok ? v6.content : "", /^full address:s:fe80::1\r\n/u, "IPv6 不追加端口");

// ────────────────────────── rdpLauncher ──────────────────────────

const tempDir = await mkdtemp(join(tmpdir(), "mode-rdp-files-"));
const killCalls: number[] = [];
const spawnCalls: string[][] = [];
/** 伪子进程：手动发事件，避免依赖真实 mstsc。 */
function createFakeChild(pid: number) {
  const listeners = new Map<string, ((arg?: unknown) => void)[]>();
  const child = {
    pid,
    on(event: string, listener: (arg?: unknown) => void) {
      const list = listeners.get(event) ?? [];
      list.push(listener);
      listeners.set(event, list);
      return child;
    },
    emit(event: string, arg?: unknown) {
      for (const listener of listeners.get(event) ?? []) {
        listener(arg);
      }
    },
  };
  return child;
}

let nextChild: ReturnType<typeof createFakeChild> | null = null;
/** 会话 id 必须逐次唯一：id 同时决定临时文件路径，复用会让两次会话撞同一文件。 */
let sessionCounter = 0;
const launcher = createRdpLauncher({
  spawn: (command, args) => {
    spawnCalls.push([command, ...args]);
    const child = createFakeChild(4242);
    nextChild = child;
    // 异步发 spawn：connect 在 spawn() 返回后才挂监听。
    setTimeout(() => child.emit("spawn"), 0);
    return child as never;
  },
  tempDir,
  killTree: async (pid) => {
    killCalls.push(pid);
  },
  logger: { info: () => undefined, warn: () => undefined },
  randomId: () => `sess-${++sessionCounter}`,
});

const connected = await launcher.connect({
  serverId: "server-a",
  name: "书房主机",
  address: "10.0.0.9",
  options: { clipboard: true },
});
assert.equal(connected.ok, true, "合法连接必须成功");
assert.equal(spawnCalls.length, 1);
assert.equal(spawnCalls[0]?.[0], "mstsc.exe", "启动的是系统 mstsc");
const rdpFile = spawnCalls[0]?.[1] ?? "";
assert.equal(rdpFile, join(tempDir, "sess-1.rdp"), "临时 .rdp 落在专用目录");
assert.equal(existsSync(rdpFile), true, "启动时 .rdp 已落盘");
assert.match(await readFile(rdpFile, "utf8"), /full address:s:10\.0\.0\.9/u, ".rdp 内容正确");

assert.equal(launcher.list().length, 1, "会话进入跟踪表");
assert.equal(launcher.list()[0]?.pid, 4242);
assert.equal(launcher.list()[0]?.name, "书房主机");

// 会话进程退出 → 跟踪移除 + 临时文件删除。
nextChild?.emit("exit", 0);
await new Promise((resolve) => setTimeout(resolve, 20));
assert.equal(launcher.list().length, 0, "退出后不再跟踪");
assert.equal(existsSync(rdpFile), false, "退出后临时 .rdp 被删除");

// 断开：调用 killTree 并移除跟踪。
const second = await launcher.connect({
  serverId: "server-b",
  name: "办公室",
  address: "nas.local",
  options: {},
});
assert.equal(second.ok, true);
const secondFile = spawnCalls[1]?.[1] ?? "";
assert.equal(secondFile, join(tempDir, "sess-2.rdp"), "第二条会话独立临时文件");
const disconnected = await launcher.disconnect("sess-2");
assert.equal(disconnected.ok, true, "断开已知会话");
assert.deepEqual(killCalls, [4242], "按 pid terminate 进程树");
assert.equal(launcher.list().length, 0);
assert.equal(existsSync(secondFile), false, "断开即删临时文件");
assert.equal((await launcher.disconnect("sess-2")).ok, false, "重复断开为 no-op");

// 非法参数：不写文件、不启动进程。
const beforeCount = spawnCalls.length;
const rejected = await launcher.connect({
  serverId: "server-c",
  name: "坏地址",
  address: "1.2.3.4\r\nalternate shell:s:cmd.exe",
  options: {},
});
assert.equal(rejected.ok, false);
assert.equal(rejected.ok === false ? rejected.reason : "", "invalid_address");
assert.equal(spawnCalls.length, beforeCount, "非法参数不得启动进程");

// 启动失败（spawn 抛错）：不残留临时文件。
const failing = createRdpLauncher({
  spawn: () => {
    throw new Error("mstsc 缺失");
  },
  tempDir,
  logger: { info: () => undefined, warn: () => undefined },
  randomId: () => "sess-fail-1",
});
const failed = await failing.connect({
  serverId: "server-d",
  name: "无 mstsc",
  address: "10.0.0.7",
  options: {},
});
assert.equal(failed.ok, false);
assert.equal(failed.ok === false ? failed.reason : "", "launch_failed");
assert.equal(existsSync(join(tempDir, "sess-fail-1.rdp")), false, "启动失败不留临时文件");

// 应用退出：删文件，但不杀会话。
await launcher.connect({ serverId: "server-e", name: "会话", address: "10.0.0.8", options: {} });
await launcher.cleanupOnQuit();
assert.equal(launcher.list().length, 0, "退出清理清空跟踪");
assert.deepEqual(killCalls, [4242], "退出**不**终止会话进程");

// ────────────────────────── rdpOptions ──────────────────────────

const optionsDir = await mkdtemp(join(tmpdir(), "mode-rdp-options-"));
const store = createRdpOptionsStore(optionsDir);
assert.deepEqual(await store.read("unknown"), {}, "未设置 = 空对象");
await store.write("server-a", { fullscreen: true, audio: "disabled", username: "bob" });
assert.deepEqual(
  await store.read("server-a"),
  { fullscreen: true, audio: "disabled", username: "bob" },
  "写读回环",
);
await store.write("server-a", {});
assert.deepEqual(await store.read("server-a"), {}, "写空对象 = 删键");
await writeFile(join(optionsDir, "rdp-options.json"), "{ not json", "utf8");
assert.deepEqual(await store.read("server-a"), {}, "损坏文件按空表降级（不抛）");

// ────────────────────────── systemRdpState ──────────────────────────

const tsOutput = [
  "",
  "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Terminal Server",
  "    fDenyTSConnections    REG_DWORD    0x0",
  "",
].join("\r\n");
assert.equal(parseRegValue(tsOutput, "fDenyTSConnections"), "0x0", "按行取值名后的值");
assert.equal(parseRegValue("", "fDenyTSConnections"), undefined, "取不到返回 undefined");
assert.equal(parseFDenyTSConnections("0x0"), true, "0x0 = 已启用");
assert.equal(parseFDenyTSConnections("0"), true);
assert.equal(parseFDenyTSConnections("1"), false, "1 = 已禁用");
assert.equal(parseFDenyTSConnections("0x1"), false);
assert.equal(parseFDenyTSConnections(undefined), undefined);
assert.equal(parseFDenyTSConnections("garbage"), undefined);

let probed = 0;
let nowMs = 1_000;
const queries: string[] = [];
const probe = createSystemRdpProbe({
  runRegQuery: async (key, valueName) => {
    probed += 1;
    queries.push(`${key}|${valueName}`);
    if (valueName === "fDenyTSConnections") {
      return tsOutput;
    }
    return "    EditionID    REG_SZ    Core\r\n";
  },
  now: () => nowMs,
  ttlMs: 1_000,
  warn: () => undefined,
});

assert.equal(probe.get(), undefined, "未探测前为未知（不发能力键）");
const first = await probe.refresh();
assert.deepEqual(first, { enabled: true, editionOk: false }, "家庭版判定为不可承载");
const probesAfterFirst = probed;
assert.deepEqual(probe.get(), { enabled: true, editionOk: false }, "TTL 内读缓存");
assert.equal(probed, probesAfterFirst, "TTL 内不得再探测");

// TTL 过期后 get() 触发后台补探测（不阻塞），且旧值仍在。
nowMs += 2_000;
assert.deepEqual(probe.get(), { enabled: true, editionOk: false }, "过期瞬间仍返回上次值");
await new Promise((resolve) => setTimeout(resolve, 20));
assert.equal(probed > probesAfterFirst, true, "过期后后台补探测");

// 探测失败：保留上一次成功值（陈旧但真实）。
const failingProbe = createSystemRdpProbe({
  runRegQuery: async () => {
    throw new Error("reg.exe 不可用");
  },
  warn: () => undefined,
});
assert.equal(await failingProbe.refresh(), undefined, "从未成功过 = 未知");

console.log("REMOTE_DESKTOP_RDP_OK");
