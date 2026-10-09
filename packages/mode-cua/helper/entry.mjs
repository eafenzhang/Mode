// Helper 入口：argv 解析 → addon 装载 → pipe 服务 → fork IPC 握手 → 父进程看门狗 → 优雅退出。
// 零依赖纯 ESM（仅 node: 内置 + 包内相对导入），Task 9 用 esbuild 打成单文件 entry.cjs。
// 握手字段逐字对齐 parseReadyMessage（windowsCuaHelperHostSupport.ts）：
// transport_ready/ready 必带非空 socketPath:string + 正整数 pid；error 必带非空顶层 message:string。
import { WINDOWS_DEV_CONTROL_PROTOCOL } from "../broker-server.js";
import { axError, loadAddon } from "./addon.mjs";
import { createHelperServer } from "./server.mjs";

const protocol = WINDOWS_DEV_CONTROL_PROTOCOL;
const PARENT_POLL_INTERVAL_MS = 2_000;
// error 握手要等 process.send 回调（消息落进内核通道）再退出；通道断裂时回调永不达，
// 保底定时器确保仍是非零退出而非事件循环空转后的 0。
const SEND_EXIT_FALLBACK_MS = 2_000;

function parseArgv(argv) {
  const options = { socketPath: undefined, parentPid: undefined };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--socket") {
      options.socketPath = argv[i + 1];
      i += 1;
    } else if (arg === "--parent-pid") {
      options.parentPid = Number(argv[i + 1]);
      i += 1;
    } else {
      // 宿主只传这两个 flag，未知参数按配置错误 fail closed（防 typo 静默起在错误管道上）。
      throw axError("internal", `unknown argument: ${String(arg)}`);
    }
  }
  if (typeof options.socketPath !== "string" || !options.socketPath.trim()) {
    throw axError("internal", "--socket is required");
  }
  if (
    options.parentPid !== undefined &&
    (!Number.isInteger(options.parentPid) || options.parentPid <= 0)
  ) {
    throw axError("internal", "--parent-pid must be a positive integer");
  }
  return options;
}

function sendControl(payload) {
  if (typeof process.send !== "function") return false;
  try {
    process.send({ protocol, ...payload });
    return true;
  } catch {
    // 父进程已亡（通道断裂）：交由父进程看门狗在 2s 内 exit(0) 兜底。
    return false;
  }
}

function failStartup(error) {
  const code = typeof error?.code === "string" && error.code ? error.code : "internal";
  const message = error instanceof Error && error.message ? error.message : String(error);
  if (typeof process.send !== "function") {
    process.exit(1);
    return;
  }
  let settled = false;
  const exit = () => {
    if (settled) return;
    settled = true;
    clearTimeout(fallback);
    process.exit(1);
  };
  // ref 定时器：回调不达时也保底退出 1，不让事件循环空转成 0。
  const fallback = setTimeout(exit, SEND_EXIT_FALLBACK_MS);
  try {
    // 回调触发 = 消息已写入内核通道，父进程在我们退出后仍能读到；
    // error 对象是附加诊断面（宿主只消费顶层 message）。
    process.send({ protocol, type: "error", message, error: { code, message } }, exit);
  } catch {
    exit();
  }
}

function main() {
  let options;
  let server;
  try {
    options = parseArgv(process.argv.slice(2));
    server = createHelperServer(loadAddon());
  } catch (error) {
    failStartup(error);
    return;
  }

  let watchdog;
  const shutdown = () => {
    if (watchdog) clearInterval(watchdog);
    server.close();
    process.exit(0);
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
  process.on("message", (message) => {
    // host 的优雅关闭通道（WindowsCuaChildLifecycle.sendShutdown 发 {protocol, type:"shutdown"}）；
    // 其他 IPC 消息忽略。Windows 上 host 先发本消息、1s 后才 kill，故必须消费它才是真优雅关。
    if (
      message &&
      typeof message === "object" &&
      message.protocol === protocol &&
      message.type === "shutdown"
    ) {
      shutdown();
    }
  });

  if (options.parentPid !== undefined) {
    // 孤儿回收兜底（配合宿主 reapOrphanedHelpers）：每 2s 探活，父亡立即退出。
    watchdog = setInterval(() => {
      try {
        process.kill(options.parentPid, 0);
      } catch (error) {
        // ESRCH = 父进程已亡 → exit(0)；EPERM = 存在但无权限，视为存活继续服务。
        if (error?.code === "ESRCH") process.exit(0);
      }
    }, PARENT_POLL_INTERVAL_MS);
  }

  server.once("error", failStartup); // listen/bind 失败（EADDRINUSE/EACCES 等）走同一 error 握手。
  try {
    server.listen(options.socketPath, () => {
      // 'listening' = bind+listen 均成功，此刻管道已绑定（transport_ready 的语义前提）；
      // 先 transport_ready 供 host 提前 resolve waitForTransport，紧发 ready 触发 healthProbe。
      sendControl({ type: "transport_ready", socketPath: options.socketPath, pid: process.pid });
      sendControl({ type: "ready", socketPath: options.socketPath, pid: process.pid });
    });
  } catch (error) {
    failStartup(error);
  }
}

main();
