// 发行包体积裁剪：去掉运行期不需要的调试/类型文件，并按目标平台裁掉别的平台原生件。
//
// 调用点：
// - scripts/build-mode.mjs：所有发行包都做通用裁剪（source map / PDB / 类型声明）；
// - scripts/build-fnos-fpk.ts：在通用裁剪之后再按 linux-x64 收窄原生件。
//
// 这里的规则只删「运行期不会被 require / 不会被浏览器请求」的文件：
// - `.map`：web 生产构建使用 hidden sourcemap，HTML 不引用它；server/agent 也不读；
// - `.pdb`：Windows 调试符号，Node 运行期不加载；
// - `.d.ts` / `.d.mts` / `.d.cts`：TypeScript 类型声明，运行期模块解析不会命中；
// - `.tsbuildinfo`：增量编译缓存。
//
// 目标平台裁剪只处理已知的「按平台分包」目录；未识别的文件一律保留，避免把运行时依赖
// 裁成「本地能跑、安装后缺文件」。

import { readdir, rm, stat } from "node:fs/promises";
import { join, relative, resolve, sep } from "node:path";

const DEBUG_FILE_PATTERN = /(?:\.map|\.pdb|\.tsbuildinfo|\.d\.ts|\.d\.mts|\.d\.cts)$/u;

/** 该相对路径是否属于运行期不需要的调试/类型文件。 */
export function isDistributionDebugFile(relativePath) {
  return DEBUG_FILE_PATTERN.test(relativePath);
}

/**
 * 该目录是否属于「非目标平台」的原生件。
 *
 * 只匹配仓库实际带进发行包的三类目录，不做宽泛猜测：
 * - `@mbears/opentui-core-<platform>-<arch>`：TUI 原生渲染库；
 * - `koffi/build/koffi/<platform>_<arch>`：FFI 原生模块；
 * - `node-pty/prebuilds/<platform>-<arch>`：终端原生模块。
 *
 * node-pty 在 Windows 上还需要 `build/Release/conpty`；非 Windows 目标保留
 * `build/Release/pty.node`，但要裁掉 Windows conpty 与构建机自己的 `build/Release`。
 */
export function shouldPruneTargetDirectory(relativePath, target) {
  const { targetPlatform, targetArch } = target;
  if (!targetPlatform || !targetArch) {
    return false;
  }

  const targetSuffix = `${targetPlatform}-${targetArch}`;
  const targetKoffi = `${targetPlatform}_${targetArch}`;

  const opentui = /^agent\/node_modules\/@mbears\/(opentui-core-[^/]+)$/u.exec(relativePath);
  if (opentui && opentui[1] !== `opentui-core-${targetSuffix}`) {
    return true;
  }

  const koffi = /^agent\/node_modules\/koffi\/build\/koffi\/([^/]+)$/u.exec(relativePath);
  if (koffi && koffi[1] !== targetKoffi) {
    return true;
  }

  const nodePtyPrebuild = /^node_modules\/node-pty\/prebuilds\/([^/]+)$/u.exec(relativePath);
  if (nodePtyPrebuild && nodePtyPrebuild[1] !== targetSuffix) {
    return true;
  }

  if (targetPlatform !== "win32") {
    if (relativePath === "node_modules/node-pty/third_party/conpty") {
      return true;
    }
    if (relativePath === "node_modules/node-pty/build/Release/conpty") {
      return true;
    }
  }

  // 交叉打包时 `build/Release` 是构建机平台的产物（例如 Windows 上打 linux 包），
  // 目标平台应改用 prebuilds/<platform>-<arch>。
  if (
    relativePath === "node_modules/node-pty/build/Release" &&
    (targetPlatform !== process.platform || targetArch !== process.arch)
  ) {
    return true;
  }

  return false;
}

async function readEntries(path) {
  try {
    return await readdir(path, { withFileTypes: true });
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

async function fileSize(path) {
  try {
    return (await stat(path)).size;
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

async function measureTree(path) {
  let bytes = 0;
  let files = 0;
  for (const entry of await readEntries(path)) {
    const full = join(path, entry.name);
    if (entry.isDirectory()) {
      const nested = await measureTree(full);
      bytes += nested.bytes;
      files += nested.files;
    } else if (entry.isFile()) {
      const size = await fileSize(full);
      if (size !== null) {
        bytes += size;
        files += 1;
      }
    }
  }
  return { bytes, files };
}

/**
 * 就地裁剪 `packageRoot`，返回删除的文件数与未压缩字节数（诊断口径）。
 * 目标平台参数省略时只做通用裁剪。
 */
export async function pruneDistribution(packageRoot, target = {}) {
  const root = resolve(packageRoot);
  const report = { files: 0, bytes: 0, directories: 0 };

  const removeDirectory = async (full) => {
    const measured = await measureTree(full);
    await rm(full, { recursive: true, force: true });
    report.files += measured.files;
    report.bytes += measured.bytes;
    report.directories += 1;
  };

  const visit = async (current) => {
    for (const entry of await readEntries(current)) {
      const full = join(current, entry.name);
      const relativePath = relative(root, full).split(sep).join("/");
      if (entry.isDirectory()) {
        if (shouldPruneTargetDirectory(relativePath, target)) {
          await removeDirectory(full);
        } else {
          await visit(full);
        }
        continue;
      }
      if (entry.isFile() && isDistributionDebugFile(relativePath)) {
        const size = await fileSize(full);
        await rm(full, { force: true });
        if (size !== null) {
          report.files += 1;
          report.bytes += size;
        }
      }
    }
  };

  await visit(root);
  return report;
}
