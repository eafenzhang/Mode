/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 skill-sync 用例，拆分会降低可核对性。 */
/**
 * 生成 `skill-sync-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/skill-sync/skillSyncService.ts`
 * - `packages/services/src/skill-sync/skillSyncArchive.ts`
 *
 * 归档字节依赖文件 mtime，生成前把所有文件/目录的 mtime 固定到同一时刻，
 * 保证脚本可重复执行；Rust 侧只比较 skills 元数据与导入结果，不逐字节比较 gzip。
 *
 * ```bash
 * pnpm skill-sync:fixtures
 * cargo test -p skill-sync-service
 * ```
 */
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "skill-sync-service", "fixtures", "skill-sync.json");

const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-skill-sync-fixtures-")));
const HOME = path.join(SANDBOX, "home");
const FIXED_TIME = new Date("2024-01-01T00:00:00.000Z");

interface FileSpec {
  path: string;
  content: string;
}

interface FixtureOp {
  kind: "listCandidates" | "remoteStatuses" | "export" | "import" | "writeAccess";
  directoryNames?: string[];
  skills?: Array<{ directoryName: string; name: string }>;
  names?: string[];
  skillIds?: string[];
  archiveBase64?: string;
  overwrite?: boolean;
}

interface FixtureCase {
  name: string;
  files: FileSpec[];
  ops: FixtureOp[];
  expected: unknown[];
  tree: FileSpec[];
}

interface SkillSyncServiceLike {
  listLocalUserSkillCandidates(): Promise<{
    candidates: Array<{ id: string; directoryName: string }>;
    maxArchiveBytes: number;
  }>;
  listRemoteUserSkillStatuses(params: {
    directoryNames: string[];
    skills?: Array<{ directoryName: string; name: string }>;
  }): Promise<unknown>;
  exportSkillsArchive(params: { skillIds: string[] }): Promise<{
    archive: Uint8Array;
    archiveBytes: number;
    skills: unknown[];
  }>;
  checkRemoteUserSkillWriteAccess(): Promise<unknown>;
  importSkillsArchive(params: { archive: Uint8Array; overwrite?: false }): Promise<unknown>;
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
  fixMtimes(root);
}

function fixMtimes(directory: string): void {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      fixMtimes(absolute);
    }
    utimesSync(absolute, FIXED_TIME, FIXED_TIME);
  }
  utimesSync(directory, FIXED_TIME, FIXED_TIME);
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
  if (typeof value === "number" && key === "archiveBytes") return "<bytes>";
  if (key === "archive") return "<archive>";
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
        files.push({
          path: childRelative,
          content: normalizeText(root, readFileSync(absolute, "utf8")),
        });
      }
    }
  };
  walk(root, "");
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

async function runOp(service: SkillSyncServiceLike, op: FixtureOp): Promise<unknown> {
  try {
    switch (op.kind) {
      case "listCandidates":
        return await service.listLocalUserSkillCandidates();
      case "remoteStatuses":
        return await service.listRemoteUserSkillStatuses({
          directoryNames: op.directoryNames ?? [],
          skills: op.skills,
        });
      case "export": {
        const listed = await service.listLocalUserSkillCandidates();
        const ids = (op.names ?? []).map((name) => {
          const found = listed.candidates.find((candidate) => candidate.directoryName === name);
          if (!found) throw new Error(`skill sync candidate not found: ${name}`);
          return found.id;
        });
        return await service.exportSkillsArchive({ skillIds: op.skillIds ?? ids });
      }
      case "import": {
        const archive = Uint8Array.from(Buffer.from(op.archiveBase64 ?? "", "base64"));
        return await service.importSkillsArchive({
          archive,
          overwrite: op.overwrite ? true : undefined,
        });
      }
      case "writeAccess":
        return await service.checkRemoteUserSkillWriteAccess();
      default:
        throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

async function runCase(
  createService: (options?: { maxArchiveBytes?: number }) => SkillSyncServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<{ fixture: FixtureCase; raw: unknown[] }> {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  materialize(SANDBOX, testCase.files);

  const service = createService();
  const expected: unknown[] = [];
  const raw: unknown[] = [];
  for (const op of testCase.ops) {
    const result = await runOp(service, op);
    raw.push(result);
    expected.push(normalizeValue(SANDBOX, result));
  }
  return {
    fixture: {
      name: testCase.name,
      files: testCase.files,
      ops: testCase.ops,
      expected,
      tree: captureTree(SANDBOX),
    },
    raw,
  };
}

function skillFile(name: string, description: string): string {
  return `---\nname: ${name}\ndescription: ${description}\n---\n${name} body\n`;
}

const SKILL_FILES: FileSpec[] = [
  { path: "home/.mode/skills/alpha/SKILL.md", content: skillFile("alpha", "mode alpha") },
  { path: "home/.mode/skills/alpha/note.txt", content: "alpha note\n" },
  { path: "home/.mode/skills/group/nested/SKILL.md", content: skillFile("nested", "nested skill") },
  { path: "home/.mode/skills/dup/SKILL.md", content: skillFile("shared", "mode shared") },
  { path: "home/.mode/skills/nofm/SKILL.md", content: "no frontmatter\n" },
  { path: "home/.mode/skills/broken/SKILL.md", content: "---\nname: [broken\n---\n" },
  { path: "home/.agents/skills/shared/SKILL.md", content: skillFile("shared", "agents shared") },
  { path: "home/.agents/skills/only/SKILL.md", content: skillFile("agentsonly", "agents only") },
  { path: "home/.agents/skills/dup/SKILL.md", content: skillFile("other", "covered dir") },
];

const IMPORT_FILES: FileSpec[] = [
  { path: "home/.mode/skills/alpha/SKILL.md", content: skillFile("alpha", "existing alpha") },
  {
    path: "home/.agents/skills/nested-other/SKILL.md",
    content: skillFile("nested", "existing nested name"),
  },
];

async function main(): Promise<void> {
  process.env.HOME = HOME;
  process.env.USERPROFILE = HOME;
  const { createSkillSyncService } =
    await import("../packages/services/src/skill-sync/skillSyncService.js");

  const candidates = await runCase(createSkillSyncService, {
    name: "candidates",
    files: SKILL_FILES,
    ops: [
      { kind: "listCandidates" },
      {
        kind: "remoteStatuses",
        directoryNames: ["alpha", "group/nested", "missing"],
        skills: [
          { directoryName: "group/nested", name: "nested" },
          { directoryName: "missing", name: "agentsonly" },
        ],
      },
      { kind: "writeAccess" },
    ],
  });

  const exported = await runCase(createSkillSyncService, {
    name: "export",
    files: SKILL_FILES,
    ops: [
      { kind: "export", names: ["alpha", "group/nested", "only"] },
      {
        kind: "remoteStatuses",
        directoryNames: ["../escape", "a/../../b"],
      },
      { kind: "export", skillIds: ["missing-id"] },
    ],
  });

  const exportedArchive = exported.raw[0] as { archive: Uint8Array } | { error?: string };
  if (!("archive" in exportedArchive)) {
    throw new Error(`export failed: ${JSON.stringify(exportedArchive)}`);
  }
  const archiveBase64 = Buffer.from(exportedArchive.archive).toString("base64");

  const imported = await runCase(createSkillSyncService, {
    name: "import",
    files: IMPORT_FILES,
    ops: [
      { kind: "import", archiveBase64 },
      { kind: "listCandidates" },
      { kind: "import", archiveBase64, overwrite: true },
    ],
  });

  const cases = [candidates.fixture, exported.fixture, imported.fixture];
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
