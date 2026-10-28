/**
 * fpk 在线升级的状态所有者（docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 只在 `MODE_FNOS_UPDATE=1` 的 fpk 部署启用；普通 `mode --web` 仍返回
 * `enabled:false`，不提供任何下载/替换能力。
 *
 * 职责边界：
 * - 本类只负责「检查 → 下载 → 校验 → 解包 → 交换载荷」；
 * - 服务进程/代理进程仍归 `cmd/main`；交换完成后只发出 `cmd/main restart`，
 *   不自己 spawn 第二个服务进程；
 * - 交换前写 `.update/in-progress`，`cmd/main start` 在启动失败时会据此回滚；
 *   交换成功后删除标记，失败时由本类立即从 backup 恢复。
 */
import { createHash, type KeyObject } from "node:crypto";
import { createWriteStream, constants } from "node:fs";
import { access, mkdir, rm, statfs } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join } from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { formatLogPrefix, MODE_VERSION, type FnosAppUpdateState } from "@mode/shared";
import { extractTarGzArchive } from "../remote/localTarGz.js";
import {
  applyStagedPayload,
  assertStagedPayloadVersion,
  readStagedPayloadVersions,
  validatePayload,
} from "./payload.js";
import { normalizeUpdatePublicKey, verifyChecksumsSignature } from "./signature.js";
import {
  buildReleaseUrls,
  compareStableVersions,
  fpkAssetName,
  parseSha256ForFile,
  parseStableVersion,
} from "./version.js";

const LOG_PREFIX = formatLogPrefix("mode-server:app-update", process.pid);
const DEFAULT_RELEASES_BASE_URL = "https://github.com/eafenzhang/Mode/releases";
const DOWNLOAD_TIMEOUT_MS = 10 * 60_000;
const CHECK_TIMEOUT_MS = 30_000;

export interface FnosAppUpdaterOptions {
  appDest?: string;
  mainScript?: string;
  currentVersion?: string;
  releasesBaseUrl?: string;
  /** 可选的发布公钥（PEM 或 32 字节裸 base64）；配置后要求 sha256.txt.sig 验签通过。 */
  updatePublicKey?: string;
  fetchImpl?: typeof fetch;
  now?: () => Date;
  log?: (...args: unknown[]) => void;
}

interface Manifest {
  version?: unknown;
}

export class FnosAppUpdater {
  private state: FnosAppUpdateState;
  private readonly appDest: string | null;
  private readonly mainScript: string | null;
  private readonly currentVersion: string;
  private readonly releasesBaseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly now: () => Date;
  private readonly log: (...args: unknown[]) => void;
  /** null = 未配置公钥（跳过验签）；`invalid` = 配了但解析失败（下载时拒绝）。 */
  private readonly updatePublicKey: KeyObject | "invalid" | null;

  constructor(options: FnosAppUpdaterOptions = {}) {
    this.appDest = options.appDest?.trim() || null;
    this.mainScript = options.mainScript?.trim() || null;
    this.currentVersion = options.currentVersion ?? MODE_VERSION;
    this.releasesBaseUrl = options.releasesBaseUrl?.trim() || DEFAULT_RELEASES_BASE_URL;
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.now = options.now ?? (() => new Date());
    this.log = options.log ?? ((...args) => console.log(LOG_PREFIX, ...args));
    const rawPublicKey = options.updatePublicKey?.trim();
    const parsedPublicKey = normalizeUpdatePublicKey(rawPublicKey);
    this.updatePublicKey = rawPublicKey ? (parsedPublicKey ?? "invalid") : null;
    this.state = {
      enabled: Boolean(this.appDest && this.mainScript),
      currentVersion: this.currentVersion,
      phase: "idle",
      canApply: false,
    };
  }

  getState(): FnosAppUpdateState {
    return { ...this.state };
  }

  async check(): Promise<FnosAppUpdateState> {
    if (!this.state.enabled) {
      return this.getState();
    }
    if (this.isBusy()) {
      return this.getState();
    }
    this.setState({ phase: "checking", error: undefined, progress: undefined });
    try {
      const urls = buildReleaseUrls(this.releasesBaseUrl, this.currentVersion);
      const manifest = await this.fetchJson<Manifest>(urls.latestManifest, CHECK_TIMEOUT_MS);
      const latestVersion = parseStableVersion(
        typeof manifest.version === "string" ? manifest.version : null,
      );
      if (!latestVersion) {
        this.setState({
          phase: "error",
          error: "invalid_release_manifest",
          checkedAt: this.now().toISOString(),
          canApply: false,
        });
        return this.getState();
      }
      const checkedAt = this.now().toISOString();
      const comparison = compareStableVersions(latestVersion, this.currentVersion);
      if (comparison <= 0) {
        this.setState({
          phase: "up-to-date",
          latestVersion,
          checkedAt,
          canApply: false,
        });
        return this.getState();
      }
      this.setState({
        phase: "available",
        latestVersion,
        checkedAt,
        error: undefined,
        canApply: false,
      });
    } catch (error) {
      this.fail("check_failed", error);
    }
    return this.getState();
  }

  async download(): Promise<FnosAppUpdateState> {
    if (!this.state.enabled) {
      return this.getState();
    }
    if (this.isBusy()) {
      return this.getState();
    }
    const latestVersion = this.state.latestVersion;
    if (this.state.phase !== "available" || !latestVersion) {
      return this.getState();
    }

    const appDest = this.appDest as string;
    const stagingRoot = join(appDest, ".update");
    const urls = buildReleaseUrls(this.releasesBaseUrl, latestVersion);
    const assetName = fpkAssetName(latestVersion);
    this.setState({
      phase: "downloading",
      progress: 0,
      error: undefined,
      latestVersion,
      canApply: false,
    });
    try {
      await rm(stagingRoot, { recursive: true, force: true });
      await mkdir(stagingRoot, { recursive: true });

      const checksums = await this.fetchText(urls.checksums, CHECK_TIMEOUT_MS);
      await this.verifyReleaseSignature(checksums, urls.checksumsSignature);
      const expectedSha = parseSha256ForFile(checksums, assetName);
      if (!expectedSha) {
        throw new Error(`checksum_missing:${assetName}`);
      }

      const fpkPath = join(stagingRoot, "package.fpk");
      const downloaded = await this.downloadFile(urls.fpk, fpkPath, appDest);
      if (downloaded.sha256 !== expectedSha) {
        await rm(fpkPath, { force: true });
        throw new Error("checksum_mismatch");
      }

      const outerDir = join(stagingRoot, "package");
      await extractTarGzArchive(fpkPath, outerDir);
      const appTgz = join(outerDir, "app.tgz");
      try {
        await access(appTgz, constants.F_OK);
      } catch {
        throw new Error("app_tgz_missing");
      }

      const appDir = join(stagingRoot, "app");
      await extractTarGzArchive(appTgz, appDir);
      await validatePayload(appDir);
      // 外壳 manifest 与载荷 package.json 必须都是 release 声明的版本；
      // 任一处不符都说明打错了包，交换后应用中心版本会与实际运行版本错位。
      assertStagedPayloadVersion(await readStagedPayloadVersions(outerDir, appDir), latestVersion);

      // 只保留解包后的载荷，释放 fpk/app.tgz 的空间。
      await rm(outerDir, { recursive: true, force: true });
      await rm(fpkPath, { force: true });

      this.setState({
        phase: "ready",
        progress: 100,
        canApply: true,
        latestVersion,
      });
      this.log(`downloaded ${assetName} (${downloaded.bytes} bytes)`);
    } catch (error) {
      await rm(stagingRoot, { recursive: true, force: true }).catch(() => undefined);
      this.fail("download_failed", error);
    }
    return this.getState();
  }

  async apply(): Promise<{ ok: boolean; restarting: boolean; error?: string }> {
    if (!this.state.enabled) {
      return { ok: false, restarting: false, error: "disabled" };
    }
    if (this.state.phase === "applying") {
      return { ok: false, restarting: false, error: "busy" };
    }
    const latestVersion = this.state.latestVersion;
    if (this.state.phase !== "ready" || !latestVersion) {
      return { ok: false, restarting: false, error: "not_ready" };
    }
    const appDest = this.appDest as string;
    const mainScript = this.mainScript as string;
    const stagingRoot = join(appDest, ".update");

    this.setState({ phase: "applying", canApply: false });
    try {
      await applyStagedPayload({ appDest, stagingRoot });
      this.log(`payload swapped for ${latestVersion}; scheduling cmd/main restart`);
      setTimeout(() => {
        try {
          const child = spawn("bash", [mainScript, "restart"], {
            detached: true,
            stdio: "ignore",
            env: process.env,
          });
          child.unref();
        } catch (error) {
          this.log("failed to spawn cmd/main restart", error);
        }
      }, 500).unref();

      return { ok: true, restarting: true };
    } catch (error) {
      this.fail("apply_failed", error);
      return {
        ok: false,
        restarting: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }

  private isBusy(): boolean {
    return (
      this.state.phase === "checking" ||
      this.state.phase === "downloading" ||
      this.state.phase === "applying"
    );
  }

  /**
   * 配置了公钥才验签；未配置时行为与旧版一致（TLS + sha256）。
   * 配了但解析失败/签名缺失/不匹配都直接拒绝下载，不降级成「无签名」。
   */
  private async verifyReleaseSignature(checksums: string, signatureUrl: string): Promise<void> {
    if (this.updatePublicKey === null) {
      return;
    }
    if (this.updatePublicKey === "invalid") {
      throw new Error("invalid_public_key");
    }
    let signature: string;
    try {
      signature = await this.fetchText(signatureUrl, CHECK_TIMEOUT_MS);
    } catch {
      throw new Error("signature_missing");
    }
    if (
      !verifyChecksumsSignature({
        checksums,
        signatureBase64: signature,
        publicKey: this.updatePublicKey,
      })
    ) {
      throw new Error("signature_invalid");
    }
  }

  private setState(patch: Partial<FnosAppUpdateState>): void {
    this.state = { ...this.state, ...patch };
  }

  private fail(code: string, error: unknown): void {
    const detail = error instanceof Error ? error.message : String(error);
    this.log(`${code}: ${detail}`);
    this.setState({
      phase: "error",
      error: code === "download_failed" && detail ? `${code}:${detail}` : code,
      progress: undefined,
      canApply: false,
      checkedAt: this.now().toISOString(),
    });
  }

  private async fetchJson<T>(url: string, timeoutMs: number): Promise<T> {
    const response = await this.fetchImpl(url, {
      redirect: "follow",
      cache: "no-store",
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) {
      throw new Error(`http_${response.status}`);
    }
    return (await response.json()) as T;
  }

  private async fetchText(url: string, timeoutMs: number): Promise<string> {
    const response = await this.fetchImpl(url, {
      redirect: "follow",
      cache: "no-store",
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) {
      throw new Error(`http_${response.status}`);
    }
    return await response.text();
  }

  private async downloadFile(
    url: string,
    targetPath: string,
    appDest: string,
  ): Promise<{ sha256: string; bytes: number }> {
    const response = await this.fetchImpl(url, {
      redirect: "follow",
      cache: "no-store",
      signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS),
    });
    if (!response.ok || !response.body) {
      throw new Error(`http_${response.status}`);
    }
    const totalBytes = Number(response.headers.get("content-length") ?? 0);
    if (totalBytes > 0) {
      const stats = await statfs(appDest);
      const freeBytes = stats.bavail * stats.bsize;
      // 峰值同时存在：下载的 fpk、外层 app.tgz、解包后的 app 载荷与旧载荷备份。
      // 4× fpk 覆盖这三份新文件 + 余量；备份是 rename，不额外占空间。
      if (freeBytes < totalBytes * 4) {
        throw new Error("insufficient_space");
      }
    }

    const hash = createHash("sha256");
    let received = 0;
    const counter = new Transform({
      transform(chunk: Buffer, _encoding, callback) {
        hash.update(chunk);
        received += chunk.length;
        callback(null, chunk);
      },
    });
    // 进度更新挂在 Transform 外：Transform 内只做哈希/计数，状态更新交给定时器，
    // 避免每个 chunk 都触发一次 React 轮询可见的快照变化。
    const progressTimer = setInterval(() => {
      if (this.state.phase !== "downloading") {
        return;
      }
      const progress =
        totalBytes > 0 ? Math.min(99, Math.floor((received / totalBytes) * 100)) : 0;
      if (progress !== this.state.progress) {
        this.setState({ progress });
      }
    }, 250);
    progressTimer.unref();
    try {
      await pipeline(
        Readable.fromWeb(response.body as import("node:stream/web").ReadableStream),
        counter,
        createWriteStream(targetPath),
      );
    } finally {
      clearInterval(progressTimer);
    }
    return { sha256: hash.digest("hex"), bytes: received };
  }

}

export function createFnosAppUpdaterFromEnv(): FnosAppUpdater {
  const enabled = process.env["MODE_FNOS_UPDATE"]?.trim() === "1";
  return new FnosAppUpdater({
    ...(enabled ? { appDest: process.env["MODE_FNOS_APPDEST"] } : {}),
    ...(enabled ? { mainScript: process.env["MODE_FNOS_MAIN_SCRIPT"] } : {}),
    ...(process.env["MODE_FNOS_UPDATE_BASE_URL"]?.trim()
      ? { releasesBaseUrl: process.env["MODE_FNOS_UPDATE_BASE_URL"].trim() }
      : {}),
    ...(process.env["MODE_FNOS_UPDATE_PUBLIC_KEY"]?.trim()
      ? { updatePublicKey: process.env["MODE_FNOS_UPDATE_PUBLIC_KEY"].trim() }
      : {}),
  });
}
