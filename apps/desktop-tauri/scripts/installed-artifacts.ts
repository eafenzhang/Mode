/**
 * 「安装目录里到底有什么、是不是产品包」这件事的判定（M3 Task 5）。
 *
 * 从 `verify-installed.ts` 抽出来的理由：那个入口已经撞上 `oxlint` 的 `max-lines`（400），
 * 而这一段是它最自然的接缝——它只读**安装目录与安装包**，不碰壳、不碰数据根，
 * 与「起壳跑一次」那条链没有共享状态。
 *
 * 三个判定各有来历（都是实测出来的，别顺手简化）：
 *
 * 1. **主 exe 名由 `mainBinaryName` 决定**（`Mode` → `Mode.exe`）。历史包曾直接使用
 *    Cargo 的 bin 名（`ModeTauri.exe`），所以这里按目录扫描 `.exe` 而不是写死名字；
 *    写死单个名字的症状是「装完了却报找不到 exe」。
 * 2. **内嵌资产名的比对基准必须与安装包同批**（`<安装包>.index.html` 快照）：
 *    renderer 产物在两次 Vite 构建之间有几百字节波动，入口 chunk 与 CSS chunk 的哈希会变，
 *    拿当前 dist 核对更早的包会得到 2/277 的**假红**。
 * 3. **覆盖重装要按字节核对**：只比对「文件在不在」无法区分「重装真跑了」与「重装什么都没做」，
 *    那样后面那次 run 跑的可能还是旧产物。
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

/** 判定回调：把结果交给调用方的汇总（本模块不自己维护清单状态）。 */
export type CheckFn = (name: string, ok: boolean, detail: string) => void;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * 找装出来的**主可执行文件**。
 *
 * 为什么扫描而不是写死 `Mode.exe`：历史 Tauri 包直接使用 Cargo 的 bin 名
 * （`ModeTauri.exe`），2026-10-27 起 `mainBinaryName` 才把它改成 `Mode.exe`；
 * 两种包都要能被同一套验收脚本识别。第一版写死单个名字的后果是
 * 「装完了却报找不到 exe」——而安装其实是成功的。
 */
export function findMainExe(installDir: string): string {
  const candidates = readdirSync(installDir)
    .filter(
      (name) => name.toLowerCase().endsWith(".exe") && !name.toLowerCase().startsWith("uninstall"),
    )
    .map((name) => join(installDir, name));
  if (candidates.length === 0) throw new Error(`${installDir} 里没有可执行文件`);
  return candidates.sort((left, right) => statSync(right).size - statSync(left).size)[0] as string;
}

/** 装过就返回主 exe，没装过返回 `null`（目录不存在也是 `null`）。 */
export function findMainExeOrNull(installDir: string): string | null {
  if (!existsSync(installDir)) return null;
  const hasMain = readdirSync(installDir).some(
    (name) => name.toLowerCase().endsWith(".exe") && !name.toLowerCase().startsWith("uninstall"),
  );
  return hasMain ? findMainExe(installDir) : null;
}

/** 静默安装到 `installDir`（NSIS：`/S` 静默、`/D` 只能放最后且**不能加引号**）。 */
export async function installSilently(installer: string, installDir: string): Promise<string> {
  mkdirSync(installDir, { recursive: true });
  console.log(`[verify] 静默安装：${installer} /S /D=${installDir}`);
  const result = spawnSync(installer, ["/S", `/D=${installDir}`], { stdio: "inherit" });
  if (result.status !== 0) throw new Error(`安装器退出码 ${String(result.status)}`);
  const deadline = Date.now() + 60_000;
  for (;;) {
    const found = readdirSync(installDir).filter((name) => name.toLowerCase().endsWith(".exe"));
    if (found.some((name) => !name.toLowerCase().startsWith("uninstall")))
      return findMainExe(installDir);
    if (Date.now() >= deadline) throw new Error(`安装后 ${installDir} 里没有可执行文件`);
    await sleep(300);
  }
}

/** 递归列出安装目录（只报前 N 项，避免报告被几千个文件淹没）。 */
export function inventory(dir: string, limit = 40): { path: string; bytes: number }[] {
  const rows: { path: string; bytes: number }[] = [];
  const walk = (current: string): void => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile()) rows.push({ path: full, bytes: statSync(full).size });
      if (rows.length >= limit) return;
    }
  };
  walk(dir);
  return rows.sort((left, right) => right.bytes - left.bytes);
}

export function sha256File(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/**
 * 从一份 HTML 里取出**入口资产名**（`/assets/xxx.js` 这类）。
 *
 * 不用 `.map((match) => match[1])`：`matchAll` 的结果里 `match[1]` 的类型是
 * `string | undefined`（`noUncheckedIndexedAccess`），直接拿去 `includes()` 是 typecheck 红。
 * 正则的捕获组里必有一个字符（`[\w.-]+`），所以 `undefined` 在这里是**不可能**的输入，
 * 用类型收窄把它排除掉，而不是靠 `!` 断言。
 */
export function assetNames(html: string): string[] {
  const names: string[] = [];
  for (const match of html.matchAll(/\/assets\/([\w.-]+\.(?:js|css))/gu)) {
    if (match[1] !== undefined) names.push(match[1]);
  }
  return names;
}

/**
 * 内嵌资产名检查：产品页入口的资产名必须在壳 exe 里逐字出现，验收台的必须不在。
 *
 * **比对基准是「与这个安装包同批的那份 `dist/index.html`」**（`measure-installer-budgets.ts`
 * 在采集时把它快照成 `<安装包>.index.html`），不是本机当前的 `dist/`。理由（M3 Task 5 实测）：
 * 三档各重跑一次 Vite 构建，而 renderer 产物在两次构建间有几百字节波动，入口 chunk 与 CSS chunk
 * 的哈希会跟着变——拿当前 dist 去核对更早那个包，会把 2/277 个名字读成「没被内嵌」，
 * 那是跨批比对的假红。没有快照时退回当前 dist 并**如实标注**基准，不假装是同批。
 */
export function checkEmbeddedFrontend(
  exe: string,
  installer: string,
  appDir: string,
  check: CheckFn,
): void {
  const snapshot = `${installer}.index.html`;
  const liveIndex = join(appDir, "dist", "index.html");
  const useSnapshot = existsSync(snapshot);
  const indexPath = useSnapshot ? snapshot : liveIndex;
  if (!existsSync(indexPath)) {
    check("产品页资产名在壳 exe 里", false, `找不到比对基准：${indexPath}`);
    return;
  }
  const baseline = useSnapshot
    ? `与安装包同批的 ${snapshot}`
    : `本机当前 ${liveIndex}（**没有同批快照，可能与这个包不同批**）`;
  const productAssets = assetNames(readFileSync(indexPath, "utf8"));
  const exeBytes = readFileSync(exe);
  const text = exeBytes.toString("latin1");
  const missing = productAssets.filter((name) => !text.includes(name));
  check(
    "产品页资产名在壳 exe 里",
    productAssets.length > 0 && missing.length === 0,
    `${baseline}：${String(productAssets.length)} 个引用资产，未命中 ${String(missing.length)} 个` +
      (missing.length > 0
        ? `（${missing.slice(0, 8).join(", ")}${missing.length > 8 ? "…" : ""}）`
        : "") +
      `；exe ${String(exeBytes.byteLength)} B`,
  );
  const benchIndex = join(appDir, "dist", "dev", "index.html");
  if (existsSync(benchIndex)) {
    const benchAssets = assetNames(readFileSync(benchIndex, "utf8"));
    const leaked = benchAssets.filter((name) => text.includes(name));
    check(
      "验收台资产名**不在**壳 exe 里",
      leaked.length === 0,
      `dev/ 的资产 ${benchAssets.join(", ") || "(无)"}；命中 ${String(leaked.length)} 个`,
    );
  } else {
    check("验收台不在 dist/ 里", true, "本机 dist/dev 不存在（发布路径已用 --no-bench 装配过）");
  }
}

/**
 * 重装一次（M3 Task 5 明确欠的那一段：`install → reinstall → run`）。
 *
 * 为什么重装是**独立**的一件事，不是「再装一遍」：NSIS 的覆盖安装走的是「先跑上一版
 * uninstaller、再落文件」那条路径，它会碰 `uninstall.exe`、注册表卸载项与已经装好的资源
 * 目录——上一次报告的那场崩溃只被**非打包的 release 壳**验证过，安装器这一整段（含重装）
 * 在本次之前从未走过。所以这里记下重装**前后**的 exe sha256 与字节数：只有「重装后 exe
 * 被换成了同一份字节」才说明重装真的落到了这份产物上，而不是「重装器什么都没做，于是后面
 * 那次 run 跑的还是旧的」。
 */
export async function reinstallAndReport(
  installer: string,
  installDir: string,
  before: { sha256: string; bytes: number },
  check: CheckFn,
): Promise<string> {
  console.log("");
  console.log("[verify] === 1b. 覆盖重装（install → reinstall → run 的那一段）===");
  const mainExe = await installSilently(installer, installDir);
  const after = { sha256: sha256File(mainExe), bytes: statSync(mainExe).size };
  check(
    "覆盖重装后主 exe 与安装包内的产物一致（同一份字节）",
    after.sha256 === before.sha256 && after.bytes === before.bytes,
    `${mainExe} 重装前 ${before.bytes} B / ${before.sha256.slice(0, 16)}… → 重装后 ${after.bytes} B / ${after.sha256.slice(0, 16)}…`,
  );
  return mainExe;
}
