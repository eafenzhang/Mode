import { MODE_DATA_ROOT_DIR_NAME } from "@mode/shared";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { maybeThrowStorageFsFault } from "../fs-fault-injection.js";

export function getDefaultSessionDbPath(): string {
  return join(homedir(), MODE_DATA_ROOT_DIR_NAME, "cli", "db", "db.sqlite");
}

export function ensureParentDir(filePath: string): void {
  const parent = dirname(filePath);
  if (!existsSync(parent)) {
    maybeThrowStorageFsFault({ operation: "mkdir", path: parent });
    mkdirSync(parent, { recursive: true });
  }
}
