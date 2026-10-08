#!/usr/bin/env node
import { loadEndpointEnv } from "./load-endpoint-env.mjs";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

import {
  copyRuntimeNodeModules,
  patchNodePtyPrebuilds,
  stageTuiRuntime,
} from "./mode-distribution/assets.mjs";
import { installScriptSource } from "./mode-distribution/installer.mjs";

const root = resolve(import.meta.dirname, "..");
const defaultOutDir = resolve(root, "dist", "mode");
const defaultBaseUrl = (await loadEndpointEnv()).MODE_DIST_BASE_URL?.trim() || "";
const packageDirName = "zcodium";
const usage = `Usage:
  pnpm build:mode
  node scripts/build-mode.mjs --skip-build
  node scripts/build-mode.mjs --version 3.3.3-dev.1
  node scripts/build-mode.mjs --out-dir dist/mode
  node scripts/build-mode.mjs --base-url http://host/mode/deps/mode/

Options:
  --skip-build        Reuse existing web/server/agent build outputs.
  --version <text>    Release version. Defaults to root package.json version.
  --out-dir <path>    Output directory. Defaults to dist/mode.
  --base-url <url>    Default install.sh download base URL.
  --help, -h          Show this help.
`;

function readArgValue(argv, arg, index) {
  if (arg.includes("=")) {
    return {
      nextIndex: index,
      value: arg.slice(arg.indexOf("=") + 1),
    };
  }
  const value = argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${arg}`);
  }
  return {
    nextIndex: index + 1,
    value,
  };
}

function parseArgs(argv) {
  const options = {
    baseUrl: defaultBaseUrl,
    help: false,
    outDir: defaultOutDir,
    skipBuild: false,
    version: undefined,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--") {
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      options.help = true;
      continue;
    }
    if (arg === "--skip-build") {
      options.skipBuild = true;
      continue;
    }
    if (arg === "--version" || arg.startsWith("--version=")) {
      const { nextIndex, value } = readArgValue(argv, arg, index);
      options.version = value;
      index = nextIndex;
      continue;
    }
    if (arg === "--out-dir" || arg.startsWith("--out-dir=")) {
      const { nextIndex, value } = readArgValue(argv, arg, index);
      options.outDir = resolve(root, value);
      index = nextIndex;
      continue;
    }
    if (arg === "--base-url" || arg.startsWith("--base-url=")) {
      const { nextIndex, value } = readArgValue(argv, arg, index);
      options.baseUrl = value.endsWith("/") ? value : `${value}/`;
      index = nextIndex;
      continue;
    }
    throw new Error(`Unknown option "${arg}". Run with --help for usage.`);
  }

  return options;
}

function commandText(command, args) {
  return [command, ...args].join(" ");
}

function run(command, args, options = {}) {
  console.log(`[mode] ${commandText(command, args)}`);
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    ...options,
  });
  if (result.error) {
    throw new Error(`${commandText(command, args)} failed: ${result.error.message}`, {
      cause: result.error,
    });
  }
  if (result.status !== 0) {
    throw new Error(`${commandText(command, args)} failed`);
  }
}

async function readJson(file) {
  return JSON.parse(await readFile(file, "utf8"));
}

async function sha256File(file) {
  const hash = createHash("sha256");
  hash.update(await readFile(file));
  return hash.digest("hex");
}

async function assertFile(file, label) {
  const fileStat = await stat(file).catch(() => null);
  if (!fileStat?.isFile()) {
    throw new Error(`Missing ${label}: ${file}`);
  }
}

async function assertDirectory(directory, label) {
  const directoryStat = await stat(directory).catch(() => null);
  if (!directoryStat?.isDirectory()) {
    throw new Error(`Missing ${label}: ${directory}`);
  }
}

async function buildOutputs(skipBuild) {
  if (skipBuild) {
    console.log("[mode] skipping build; reusing existing outputs");
    return;
  }

  run("pnpm", ["--filter", "@mode/cli...", "build"]);
  await rm(resolve(root, "packages", "server", "dist"), {
    force: true,
    recursive: true,
  });
  run("pnpm", ["--filter", "@mode/server", "build"]);
  run("pnpm", ["--filter", "@mode/web", "build"]);
}

async function stageModePackage({ packageRoot, version }) {
  const webDist = resolve(root, "packages", "web", "dist");
  const serverDist = resolve(root, "packages", "server", "dist");
  const agentBundle = resolve(root, "apps", "mode-cli", "packages", "cli", "dist", "mode.cjs");
  const agentProvider = resolve(root, "apps/mode-cli/packages/cli/dist/provider");

  await assertDirectory(webDist, "web dist");
  await assertDirectory(serverDist, "server dist");
  await assertFile(resolve(serverDist, "entry-http.js"), "server HTTP entry");
  await assertFile(agentBundle, "agent app-server bundle");
  await assertFile(resolve(agentProvider, "mode-builtin.json"), "Agent provider config");

  await rm(packageRoot, {
    force: true,
    recursive: true,
  });
  await mkdir(packageRoot, {
    recursive: true,
  });

  await cp(webDist, resolve(packageRoot, "web"), {
    recursive: true,
  });
  await cp(serverDist, resolve(packageRoot, "server"), {
    recursive: true,
  });
  await mkdir(resolve(packageRoot, "agent"), {
    recursive: true,
  });
  await cp(agentBundle, resolve(packageRoot, "agent", "mode.cjs"));
  // TUI 入口通过真正的 CLI 路径定位伴随配置；只复制 JS 会在仓库外启动失败。
  await cp(agentProvider, resolve(packageRoot, "agent/provider"), { recursive: true });
  await cp(
    resolve(root, "apps/mode-cli/packages/cli/dist/THIRD-PARTY-NOTICES.md"),
    resolve(packageRoot, "agent/THIRD-PARTY-NOTICES.md"),
  );
  // 许可与声明材料随包分发：MIT（本仓库）、Apache-2.0（上游）与 NOTICE 说明。
  for (const licenseFile of ["LICENSE", "LICENSE-APACHE", "NOTICE.md", "NOTICE.zh-CN.md"]) {
    await cp(resolve(root, licenseFile), resolve(packageRoot, licenseFile));
  }
  await chmod(resolve(packageRoot, "agent", "mode.cjs"), 0o755);

  await stageTuiRuntime(packageRoot);
  await copyRuntimeNodeModules(packageRoot);
  await patchNodePtyPrebuilds(packageRoot);

  await mkdir(resolve(packageRoot, "bin"), {
    recursive: true,
  });
  const runner = resolve(packageRoot, "bin", "mode.mjs");
  await cp(resolve(root, "scripts/mode-distribution/runner.mjs"), runner);
  await chmod(runner, 0o755);

  await writeFile(
    resolve(packageRoot, "package.json"),
    JSON.stringify(
      {
        name: "mode-runtime",
        private: true,
        type: "module",
        version,
      },
      null,
      2,
    ),
  );
}

async function createTarball({ packageParent, releaseDir, tarballName }) {
  await mkdir(releaseDir, {
    recursive: true,
  });
  const tarball = resolve(releaseDir, tarballName);
  await rm(tarball, {
    force: true,
  });
  run("tar", ["-czf", tarball, "-C", packageParent, packageDirName]);
  return tarball;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (!options.help && !options.baseUrl)
    throw new Error("Configure MODE_DIST_BASE_URL in .env or pass --base-url");
  if (options.help) {
    console.log(usage);
    return;
  }

  const rootPackageJson = await readJson(resolve(root, "package.json"));
  const version = options.version ?? rootPackageJson.version;
  if (!version || typeof version !== "string") {
    throw new Error("Unable to resolve Mode version.");
  }

  await buildOutputs(options.skipBuild);

  const outDir = options.outDir;
  const workDir = resolve(outDir, ".work");
  const packageParent = workDir;
  const packageRoot = resolve(packageParent, packageDirName);
  const releaseDir = resolve(outDir, "releases", version);
  const tarballName = `${packageDirName}-${version}.tar.gz`;

  await rm(workDir, {
    force: true,
    recursive: true,
  });
  await stageModePackage({
    packageRoot,
    version,
  });
  const tarball = await createTarball({
    packageParent,
    releaseDir,
    tarballName,
  });
  const sha256 = await sha256File(tarball);
  await writeFile(resolve(releaseDir, "sha256.txt"), `${sha256}  ${tarballName}\n`);

  await writeFile(
    resolve(outDir, "latest.json"),
    JSON.stringify(
      {
        baseUrl: options.baseUrl,
        createdAt: new Date().toISOString(),
        name: "zcodium",
        sha256,
        tarball: tarballName,
        version,
      },
      null,
      2,
    ),
  );
  const installScript = resolve(outDir, "install.sh");
  await writeFile(installScript, installScriptSource(options.baseUrl));
  await chmod(installScript, 0o755);
  await rm(workDir, {
    force: true,
    recursive: true,
  });

  console.log(`[mode] release directory: ${outDir}`);
  console.log(`[mode] tarball: ${tarball}`);
  console.log(`[mode] sha256: ${sha256}`);
}

await main();
