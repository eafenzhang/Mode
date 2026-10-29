/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明读侧用例，拆分会降低可核对性。 */
/**
 * 生成 `workspace-hooks-service` 读侧（`loadHooks`）的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/hooks/hooksService.ts`
 * - `packages/services/src/hooks/workspaceHookSettingsModel.ts`
 *
 * ```bash
 * pnpm hooks-load:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p workspace-hooks-service"
 * ```
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { createHooksService } from "../packages/services/src/hooks/hooksService.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "workspace-hooks-service", "fixtures", "hooks-load.json");
const DISCOVERED_AT = "2026-10-30T00:00:00.000Z";

interface FileSpec {
  path: string;
  content: string;
}

const PROJECT_CONFIG = JSON.stringify(
  {
    hooks: {
      enabled: true,
      events: {
        PreToolUse: [
          { matcher: "Bash", hooks: [{ type: "command", command: "echo proj", statusMessage: "检查" }] },
          { hooks: [{ type: "process", command: "node", args: ["x.js"], timeoutMs: 2500 }] },
        ],
        Stop: [{ hooks: [{ type: "command", command: "echo stop", env: { A: "1" } }] }],
      },
    },
  },
  null,
  2,
);

const USER_CONFIG = JSON.stringify(
  {
    hooks: {
      events: { SessionStart: [{ hooks: [{ type: "command", command: "echo user" }] }] },
    },
  },
  null,
  2,
);

const LEGACY_PROJECT = JSON.stringify(
  {
    hooks: {
      PreToolUse: [
        { matcher: "Read", hooks: [{ type: "command", command: "echo legacy", timeoutMs: 1500 }] },
        { hooks: [{ type: "script", command: "ignored" }] },
      ],
      UnknownEvent: [{ hooks: [{ type: "command", command: "ignored" }] }],
    },
  },
  null,
  2,
);

const LEGACY_USER = JSON.stringify(
  { hooks: { Stop: [{ hooks: [{ type: "command", command: "echo claude-user" }] }] } },
  null,
  2,
);

const BASE_FILES: FileSpec[] = [
  { path: "ws/.mode/config.json", content: PROJECT_CONFIG },
  { path: "ws/.agents/settings.json", content: LEGACY_PROJECT },
  { path: "ws/.claude/settings.json", content: LEGACY_PROJECT },
  { path: "home/.mode/cli/config.json", content: USER_CONFIG },
  { path: "home/.agents/settings.json", content: LEGACY_USER },
  { path: "home/.claude/settings.json", content: LEGACY_USER },
];

function normalizeWith(root: string, value: unknown): unknown {
  if (typeof value === "string") {
    const forward = root.split(path.sep).join("/");
    let replaced = value.replaceAll(root, "<root>");
    replaced = replaced.replaceAll(forward, "<root>");
    return replaced.split(path.sep).join("/");
  }
  if (Array.isArray(value)) return value.map((item) => normalizeWith(root, item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] = normalizeWith(root, child);
    }
    return out;
  }
  return value;
}

function materialize(root: string, files: FileSpec[]): void {
  for (const file of files) {
    const target = path.join(root, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content);
  }
}

async function load(root: string) {
  const home = path.join(root, "home");
  const previousHome = process.env.HOME;
  const previousProfile = process.env.USERPROFILE;
  process.env.HOME = home;
  process.env.USERPROFILE = home;
  try {
    return await createHooksService().loadHooks({ workspacePath: path.join(root, "ws") });
  } finally {
    if (previousHome === undefined) delete process.env.HOME;
    else process.env.HOME = previousHome;
    if (previousProfile === undefined) delete process.env.USERPROFILE;
    else process.env.USERPROFILE = previousProfile;
  }
}

async function runCase(name: string, files: FileSpec[]) {
  const root = mkdtempSync(path.join(tmpdir(), "zc-hooks-load-"));
  try {
    materialize(root, files);
    const result = await load(root);
    return {
      name,
      files,
      discoveredAt: DISCOVERED_AT,
      expected: normalizeWith(root, {
        hooks: result.hooks,
        hooksEnabled: result.hooksEnabled,
        snapshot: result.workspaceHookSnapshot
          ? { ...result.workspaceHookSnapshot, discoveredAt: "<discoveredAt>" }
          : null,
        trustStoreCorrupt: result.trustStoreCorrupt === true,
      }),
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

async function trustedCase() {
  const root = mkdtempSync(path.join(tmpdir(), "zc-hooks-load-trust-"));
  try {
    materialize(root, BASE_FILES);
    const first = await load(root);
    const entry = first.workspaceHookSnapshot?.hooks[0];
    if (!entry) throw new Error("snapshot entry missing");
    // 服务层用原生分隔符比较 identity，这里必须与它一致；落盘前再统一归一化成 <root>。
    const identity = path.join(root, "ws");
    const trustFile = path.join(root, "home", ".mode", "security", "workspace-hook-trust-v1.json");
    mkdirSync(path.dirname(trustFile), { recursive: true });
    writeFileSync(
      trustFile,
      JSON.stringify(
        {
          schemaVersion: 1,
          records: [
            {
              workspaceIdentity: identity,
              hookDeclarationDigest: entry.hookDeclarationDigest,
              digestAlgorithm: "sha256",
              decision: "trusted",
              grantedAt: "2026-10-30T00:00:00.000Z",
              bundleDigestAtGrant: first.workspaceHookSnapshot?.bundleDigest,
              eventAtGrant: entry.event,
              displayCommandAtGrant: entry.command,
              sourcePathAtGrant: entry.sourceRelativePath,
              sourceDiscoveryOrderAtGrant: 0,
              matcherAtGrant: entry.matcher ?? null,
              matcherIndexAtGrant: entry.matcherIndex,
              hookIndexAtGrant: entry.hookIndex,
            },
          ],
        },
        null,
        2,
      ),
    );
    // 记录时把原生 identity 换成 <root>：Rust 侧用正斜杠 identity，归一化后必须能对上。
    const trustJson = JSON.parse(readFileSync(trustFile, "utf8")) as {
      records: Array<{ workspaceIdentity: string }>;
    };
    trustJson.records[0]!.workspaceIdentity = "<root>/ws";
    const files: FileSpec[] = [
      ...BASE_FILES,
      {
        path: "home/.mode/security/workspace-hook-trust-v1.json",
        content: JSON.stringify(trustJson, null, 2),
      },
    ];
    const result = await load(root);
    return {
      name: "trusted",
      files,
      discoveredAt: DISCOVERED_AT,
      expected: normalizeWith(root, {
        hooks: result.hooks,
        hooksEnabled: result.hooksEnabled,
        snapshot: result.workspaceHookSnapshot
          ? { ...result.workspaceHookSnapshot, discoveredAt: "<discoveredAt>" }
          : null,
        trustStoreCorrupt: result.trustStoreCorrupt === true,
      }),
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

async function main(): Promise<void> {
  const cases = [
    await runCase("basic", BASE_FILES),
    await trustedCase(),
    await runCase("corrupt-trust", [
      ...BASE_FILES,
      { path: "home/.mode/security/workspace-hook-trust-v1.json", content: "{ not json" },
    ]),
    await runCase("invalid-user-config", [
      { path: "ws/.mode/config.json", content: PROJECT_CONFIG },
      { path: "home/.mode/cli/config.json", content: JSON.stringify({ hooks: { events: [] } }) },
    ]),
    await runCase("empty", []),
  ];
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
}

await main();
