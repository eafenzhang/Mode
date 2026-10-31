/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 mcp-sync 用例，拆分会降低可核对性。 */
/**
 * 生成 `mcp-sync-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/mcp-sync/mcpSyncService.ts`
 *
 * ```bash
 * pnpm mcp-sync:fixtures
 * cargo test -p mcp-sync-service
 * ```
 */
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "mcp-sync-service", "fixtures", "mcp-sync.json");

const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-mcp-sync-fixtures-")));
const HOME = path.join(SANDBOX, "home");
const WORKSPACE = path.join(SANDBOX, "ws");

interface FileSpec {
  path: string;
  content: string;
}

interface DirectoryLocation {
  source: string;
  scope: string;
  directoryPath: string;
  projectPath?: string;
}

interface FixtureOp {
  kind:
    | "load"
    | "save"
    | "candidates"
    | "remoteStatuses"
    | "export"
    | "checkWriteAccess"
    | "import";
  useWorkspace?: boolean;
  payload?: {
    action: "upsert" | "delete" | "set-enabled";
    source?: string;
    name: string;
    config?: unknown;
    enabled?: boolean;
    useWorkspace?: boolean;
    location?: DirectoryLocation;
  };
  names?: string[];
  serverIds?: string[];
  importServers?: Array<{
    id: string;
    name: string;
    config: unknown;
    enabled: boolean;
    source: string;
    path: string;
  }>;
  overwrite?: boolean;
}

interface FixtureCase {
  name: string;
  files: FileSpec[];
  ops: FixtureOp[];
  expected: unknown[];
  tree: FileSpec[];
}

interface McpSyncServiceLike {
  loadMcpFromUserDirectory(request?: { workspacePath?: string }): Promise<unknown>;
  saveMcpToUserDirectory(payload: {
    action: "upsert" | "delete" | "set-enabled";
    source?: string;
    name: string;
    config?: unknown;
    enabled?: boolean;
    projectPath?: string;
    location?: DirectoryLocation;
  }): Promise<void>;
  listLocalUserMcpCandidates(): Promise<unknown>;
  listRemoteUserMcpStatuses(params: { names: string[] }): Promise<unknown>;
  exportMcpServers(params: { serverIds: string[] }): Promise<unknown>;
  checkRemoteUserMcpWriteAccess(): Promise<unknown>;
  importMcpServers(params: {
    servers: Array<{
      id: string;
      name: string;
      config: unknown;
      enabled: boolean;
      source: string;
      path: string;
    }>;
    localHomeDir: string;
    localWorkspacePath?: string;
    remoteWorkspacePath?: string;
    overwrite?: false;
  }): Promise<unknown>;
}

function forwardRoot(): string {
  return SANDBOX.split(path.sep).join("/");
}

function materialize(root: string, files: FileSpec[]): void {
  for (const file of files) {
    const target = path.join(root, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content.replaceAll("<root>", forwardRoot()));
  }
}

function normalizeText(root: string, text: string): string {
  const native = root;
  const forward = root.split(path.sep).join("/");
  const escaped = native.split("\\").join("\\\\");
  let replaced = text.replaceAll(escaped, "<root>");
  replaced = replaced.replaceAll(native, "<root>");
  replaced = replaced.replaceAll(forward, "<root>");
  if (path.sep === "\\") {
    replaced = replaced.split("\\\\").join("/").split("\\").join("/");
  }
  return replaced;
}

const SHA256 = /^[0-9a-f]{64}$/u;

function normalizeValue(root: string, value: unknown, key?: string): unknown {
  if (typeof value === "string") {
    return SHA256.test(value) && key === "id" ? "<sha256>" : normalizeText(root, value);
  }
  if (Array.isArray(value)) return value.map((item) => normalizeValue(root, item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value as Record<string, unknown>)) {
      out[childKey] = normalizeValue(root, child, childKey);
    }
    return out;
  }
  return value;
}

function sortDeep(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortDeep);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value as Record<string, unknown>).sort()) {
      out[key] = sortDeep((value as Record<string, unknown>)[key]);
    }
    return out;
  }
  return value;
}

function normalizeJsonFile(root: string, content: string): string {
  return normalizeText(root, JSON.stringify(sortDeep(JSON.parse(content)), null, 2));
}

function captureTree(root: string): FileSpec[] {
  const files: FileSpec[] = [];
  const walk = (directory: string, relative: string): void => {
    const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      left.name.localeCompare(right.name),
    );
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      const childRelative = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        walk(absolute, childRelative);
      } else if (entry.isFile()) {
        const raw = readFileSync(absolute, "utf8");
        files.push({
          path: childRelative,
          content: entry.name.endsWith(".json")
            ? normalizeJsonFile(root, raw)
            : normalizeText(root, raw),
        });
      }
    }
  };
  walk(root, "");
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

async function resolveCandidateIds(
  service: McpSyncServiceLike,
  names: string[],
): Promise<string[]> {
  const listed = (await service.listLocalUserMcpCandidates()) as {
    candidates: Array<{ id: string; name: string }>;
  };
  return names.map((name) => {
    const found = listed.candidates.find((candidate) => candidate.name === name);
    if (!found) throw new Error(`mcp sync candidate not found: ${name}`);
    return found.id;
  });
}

async function runOp(service: McpSyncServiceLike, op: FixtureOp): Promise<unknown> {
  try {
    switch (op.kind) {
      case "load":
        return await service.loadMcpFromUserDirectory(
          op.useWorkspace ? { workspacePath: WORKSPACE } : {},
        );
      case "save":
        return await service
          .saveMcpToUserDirectory({
            action: op.payload!.action,
            source: op.payload!.source,
            name: op.payload!.name,
            config: op.payload!.config,
            enabled: op.payload!.enabled,
            ...(op.payload!.location ? { location: op.payload!.location } : {}),
            ...(op.payload!.useWorkspace ? { projectPath: WORKSPACE } : {}),
          })
          .then(() => ({ ok: true }));
      case "candidates":
        return await service.listLocalUserMcpCandidates();
      case "remoteStatuses":
        return await service.listRemoteUserMcpStatuses({ names: op.names ?? [] });
      case "export": {
        const serverIds = op.serverIds ?? (await resolveCandidateIds(service, op.names ?? []));
        return await service.exportMcpServers({ serverIds });
      }
      case "checkWriteAccess":
        return await service.checkRemoteUserMcpWriteAccess();
      case "import":
        return await service.importMcpServers({
          servers: op.importServers ?? [],
          localHomeDir: HOME,
          localWorkspacePath: WORKSPACE,
          remoteWorkspacePath: "/srv/ws",
          overwrite: op.overwrite ? true : undefined,
        });
      default:
        throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

async function runCase(
  createMcpSyncService: () => McpSyncServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<FixtureCase> {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  materialize(SANDBOX, testCase.files);

  const service = createMcpSyncService();
  const expected: unknown[] = [];
  for (const op of testCase.ops) {
    expected.push(normalizeValue(SANDBOX, await runOp(service, op)));
  }
  return {
    name: testCase.name,
    files: testCase.files,
    ops: testCase.ops,
    expected,
    tree: captureTree(SANDBOX),
  };
}

const MODE_USER_CONFIG: FileSpec = {
  path: "home/.mode/cli/config.json",
  content: JSON.stringify(
    {
      theme: "dark",
      mcp: {
        servers: {
          alpha: { command: "npx", args: ["-y", "pkg"] },
          disabled: { command: "node", args: ["server.js"], enabled: false },
          legacy: { command: "legacy", enable: true },
          legacyoff: { command: "off", enable: false, enabled: true },
        },
      },
    },
    null,
    2,
  ),
};

async function main(): Promise<void> {
  process.env.HOME = HOME;
  process.env.USERPROFILE = HOME;
  const { createMcpSyncService } =
    await import("../packages/services/src/mcp-sync/mcpSyncService.js");

  const cases: FixtureCase[] = [
    await runCase(createMcpSyncService, {
      name: "load-user",
      files: [MODE_USER_CONFIG],
      ops: [{ kind: "load" }],
    }),
    await runCase(createMcpSyncService, {
      name: "load-workspace-preferred",
      files: [
        MODE_USER_CONFIG,
        {
          path: "ws/.mode/config.json",
          content: JSON.stringify(
            { mcp: { servers: { wsone: { command: "node", args: ["ws.js"] } } } },
            null,
            2,
          ),
        },
        {
          path: "ws/.agents/mcp.json",
          content: JSON.stringify({ mcpServers: { wsagents: { command: "node" } } }, null, 2),
        },
      ],
      ops: [{ kind: "load", useWorkspace: true }, { kind: "load" }],
    }),
    await runCase(createMcpSyncService, {
      name: "load-agents-fallback",
      files: [
        {
          path: "home/.agents/mcp.json",
          content: JSON.stringify(
            { mcpServers: { fromagents: { command: "node", args: ["a.js"] } } },
            null,
            2,
          ),
        },
      ],
      ops: [{ kind: "load" }, { kind: "candidates" }],
    }),
    await runCase(createMcpSyncService, {
      name: "save-actions",
      files: [MODE_USER_CONFIG],
      ops: [
        {
          kind: "save",
          payload: {
            action: "upsert",
            name: "newone",
            config: { command: "npx", args: ["-y", "new"] },
          },
        },
        { kind: "load" },
        {
          kind: "save",
          payload: { action: "set-enabled", name: "alpha", enabled: false },
        },
        { kind: "load" },
        { kind: "save", payload: { action: "delete", name: "alpha" } },
        { kind: "load" },
        { kind: "save", payload: { action: "set-enabled", name: "alpha" } },
        {
          kind: "save",
          payload: { action: "upsert", name: "nocfg" },
        },
        {
          kind: "save",
          payload: {
            action: "set-enabled",
            name: "alpha",
            enabled: true,
            location: { source: "other", scope: "user", directoryPath: "<root>/x" },
          },
        },
      ],
    }),
    await runCase(createMcpSyncService, {
      name: "candidates-export",
      files: [MODE_USER_CONFIG],
      ops: [
        { kind: "candidates" },
        { kind: "export", names: ["alpha", "disabled"] },
        { kind: "export", serverIds: ["missing-id"] },
      ],
    }),
    await runCase(createMcpSyncService, {
      name: "remote-statuses",
      files: [MODE_USER_CONFIG],
      ops: [{ kind: "remoteStatuses", names: ["alpha", "Alpha ", "missing"] }],
    }),
    await runCase(createMcpSyncService, {
      name: "write-access",
      files: [],
      ops: [{ kind: "checkWriteAccess" }],
    }),
    await runCase(createMcpSyncService, {
      name: "import",
      files: [
        {
          path: "home/.mode/cli/config.json",
          content: JSON.stringify(
            { mcp: { servers: { "existing-target": { command: "node", args: ["t.js"] } } } },
            null,
            2,
          ),
        },
        {
          path: "home/.agents/mcp.json",
          content: JSON.stringify(
            { mcpServers: { "existing-agents": { command: "node", args: ["a.js"] } } },
            null,
            2,
          ),
        },
      ],
      ops: [
        {
          kind: "import",
          importServers: [
            {
              id: "server-1",
              name: "newone",
              config: { command: "npx", args: ["-y", "pkg"] },
              enabled: true,
              source: "mode",
              path: "",
            },
            {
              id: "server-2",
              name: "existing-target",
              config: { command: "x" },
              enabled: true,
              source: "mode",
              path: "",
            },
            {
              id: "server-3",
              name: "existing-agents",
              config: { command: "y" },
              enabled: true,
              source: "agents",
              path: "",
            },
            {
              id: "server-4",
              name: "filesystem",
              config: {
                command: "npx",
                args: [
                  "-y",
                  "@modelcontextprotocol/server-filesystem",
                  "<root>/ws/docs",
                  "<root>/home/data",
                ],
              },
              enabled: false,
              source: "mode",
              path: "",
            },
          ],
        },
        { kind: "load" },
        {
          kind: "import",
          importServers: [],
          overwrite: true,
        },
      ],
    }),
  ];

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
