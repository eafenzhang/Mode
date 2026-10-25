/**
 * Host sidecar 的文件日志 sink（2026-10-25 落地）。
 *
 * 背景：旧 Electron main 随旧线 EOL 后，`~/.mode/v2/logs/*.log` 停止更新——Host sidecar
 * 的日志只写壳的 stderr，安装版（无控制台）上等于不可见，Host 进程死亡（wedge）时零现场
 * 可查。本模块把 Host 既有 `log()` 出口镜像一份到数据根下的日志文件，best-effort：
 * 任一 IO 失败只保留 stderr，绝不影响 Host 运行。
 *
 * 命名 `host-YYYY-MM-DD.log`：与旧 Electron 线的 `YYYY-MM-DD.log` 区分（那套语义已终止），
 * 同日追加、跨日滚动（按本地时区日期取文件名；跨日后的首条写自然落到新文件）。
 */
import { appendFile, mkdir, readdir, unlink } from "node:fs/promises";
import { join } from "node:path";

import { MODE_DATA_ROOT_DIR_NAME } from "./dataRoot.js";

/** 日志目录：`<dataRoot>/v2/logs`（storageCatalog 的 logs 域同一位置）。 */
export function hostLogDir(dataRoot: string): string {
  return join(dataRoot, "v2", "logs");
}

/** 当天的日志文件名（本地时区）。 */
export function hostLogFileName(date = new Date()): string {
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `host-${date.getFullYear()}-${month}-${day}.log`;
}

/** 保留天数：超过即在启动时清理（Host 日志是诊断面，不是审计记录）。 */
export const HOST_LOG_RETENTION_DAYS = 7;

/**
 * 判断一个日志文件名是否超期可清理。只认 `host-YYYY-MM-DD.log` 形态，
 * 其它文件（旧 Electron 线的日期文件、用户放置物）一律不动。
 */
export function isExpiredHostLogFile(fileName: string, now = new Date()): boolean {
  const match = /^host-(\d{4})-(\d{2})-(\d{2})\.log$/.exec(fileName);
  if (!match) {
    return false;
  }
  const fileDate = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  if (Number.isNaN(fileDate.getTime())) {
    return false;
  }
  const ageMs = now.getTime() - fileDate.getTime();
  return ageMs > HOST_LOG_RETENTION_DAYS * 24 * 60 * 60 * 1000;
}

export interface HostLogFileSink {
  /** 追加一行（调用方已带换行）；IO 失败静默丢弃——日志不能反向影响业务。 */
  write(line: string): void;
}

/**
 * 建 sink：目录建不出来/首批写入失败时返回仍可安全调用的 no-op sink
 * （写失败一次后不再重试，避免每条日志都打一次失败的 stderr 噪音）。
 */
export async function createHostLogFileSink(dataRoot: string): Promise<HostLogFileSink> {
  const dir = hostLogDir(dataRoot);
  try {
    await mkdir(dir, { recursive: true });
  } catch {
    return { write: () => {} };
  }
  let broken = false;
  const writeDate = new Date();
  let writeChain: Promise<void> = Promise.resolve();
  // 启动时顺手清理超期文件（best-effort；失败不影响本进程写日志）。
  void (async () => {
    try {
      for (const name of await readdir(dir)) {
        if (isExpiredHostLogFile(name)) {
          await unlink(join(dir, name));
        }
      }
    } catch {
      // 清理失败无所谓：下次启动再清。
    }
  })();
  return {
    write(line: string): void {
      if (broken) {
        return;
      }
      // 跨日滚动：写时按当前日期取文件名（同日追加、跨日自然切文件）。
      const file = join(dir, hostLogFileName(writeDate));
      // 串行写链：并发的 appendFile 在 Windows 上完成顺序不保证（实测出现后行先行落盘），
      // 日志顺序是可读性的前提——每条写等上一条完成后再发起。
      writeChain = writeChain
        .then(() => appendFile(file, line.endsWith("\n") ? line : `${line}\n`, "utf8"))
        .catch(() => {
          broken = true;
        });
    },
  };
}
