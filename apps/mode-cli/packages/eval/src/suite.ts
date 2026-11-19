// Suite 加载：单文件 `{schemaVersion,cases}` 或目录（每个行为夹具一份，与 oracle fixtures 同形）。
// 目录里的协议夹具（`direction`/`frame`）不是行为用例，跳过而不是让整个 suite 失败。
import { readFile, readdir, stat } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

import { CaseSchema, SuiteSchema, type EvalCase, type Suite } from "./types.js";

export class SuiteLoadError extends Error {
  constructor(
    message: string,
    readonly path: string,
  ) {
    super(message);
    this.name = "SuiteLoadError";
  }
}

export async function loadSuite(path: string): Promise<Suite> {
  const absolute = resolve(path);
  const info = await stat(absolute);
  if (info.isDirectory()) return loadSuiteDirectory(absolute);
  return loadSuiteFile(absolute);
}

async function loadSuiteFile(path: string): Promise<Suite> {
  const parsed = await readJson(path);
  if (isRecord(parsed) && Array.isArray(parsed.cases)) {
    const suite = SuiteSchema.parse(parsed);
    assertUnique(suite.cases, path);
    return suite;
  }
  const single = CaseSchema.parse(parsed);
  return {
    schemaVersion: 1,
    name: basename(path, ".json"),
    cases: [single],
  };
}

async function loadSuiteDirectory(dir: string): Promise<Suite> {
  const entries = (await readdir(dir)).filter((entry) => entry.endsWith(".json")).sort();
  const cases: EvalCase[] = [];
  for (const entry of entries) {
    const path = join(dir, entry);
    const info = await stat(path);
    if (!info.isFile()) continue;
    const parsed = await readJson(path);
    if (isRecord(parsed) && Array.isArray(parsed.cases)) {
      cases.push(...SuiteSchema.parse(parsed).cases);
      continue;
    }
    if (!isBehaviorCase(parsed)) continue;
    cases.push(CaseSchema.parse(parsed));
  }
  if (cases.length === 0) {
    throw new SuiteLoadError(`no behavior cases found in directory`, dir);
  }
  assertUnique(cases, dir);
  return { schemaVersion: 1, name: basename(dir), cases };
}

function isBehaviorCase(value: unknown): boolean {
  return isRecord(value) && typeof value.prompt === "string" && Array.isArray(value.script);
}

async function readJson(path: string): Promise<unknown> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    throw new SuiteLoadError(`cannot read suite file: ${messageOf(error)}`, path);
  }
  try {
    return JSON.parse(text) as unknown;
  } catch (error) {
    throw new SuiteLoadError(`invalid JSON: ${messageOf(error)}`, path);
  }
}

function assertUnique(cases: readonly EvalCase[], path: string): void {
  const seen = new Set<string>();
  for (const item of cases) {
    if (seen.has(item.case)) {
      throw new SuiteLoadError(`duplicate case id: ${item.case}`, path);
    }
    seen.add(item.case);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
