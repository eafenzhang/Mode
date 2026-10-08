import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ModeStdioTapDevState } from "@mode/shared";
import { getAppConfigDir } from "#src/paths.js";
import { isEffectiveDevelopmentNodeEnv } from "#src/runtime-tools/nodeEnv.js";

interface ModeStdioTapStateFile {
  enabled?: boolean;
}

function isModeStdioTapDevVisible(): boolean {
  return isEffectiveDevelopmentNodeEnv();
}

function getModeStdioTapDevDir(): string {
  return join(getAppConfigDir(), "dev");
}

export function getModeStdioTapDevLogDir(): string {
  return join(getModeStdioTapDevDir(), "stdio-traffic");
}

function getModeStdioTapDevStatePath(): string {
  return join(getModeStdioTapDevDir(), "mode-stdio-tap.json");
}

function readStateFile(path: string): ModeStdioTapStateFile {
  if (!existsSync(path)) {
    return {};
  }

  try {
    const parsed = JSON.parse(readFileSync(path, "utf-8")) as unknown;
    return parsed && typeof parsed === "object" ? (parsed as ModeStdioTapStateFile) : {};
  } catch {
    return {};
  }
}

export function readModeStdioTapDevState(): ModeStdioTapDevState {
  const visible = isModeStdioTapDevVisible();
  const statePath = getModeStdioTapDevStatePath();
  const fileState = readStateFile(statePath);
  return {
    enabled: visible && fileState.enabled === true,
    visible,
    logDir: getModeStdioTapDevLogDir(),
    statePath,
  };
}

export function setModeStdioTapDevEnabled(enabled: boolean): ModeStdioTapDevState {
  const visible = isModeStdioTapDevVisible();
  const statePath = getModeStdioTapDevStatePath();
  mkdirSync(getModeStdioTapDevDir(), { recursive: true });
  writeFileSync(
    statePath,
    `${JSON.stringify(
      {
        // 开发态 stdio 抓包是高频原始协议帧，只能通过显式开关写旁路文件，避免误进生产日志。
        enabled: visible && enabled,
        updatedAt: new Date().toISOString(),
      },
      null,
      2,
    )}\n`,
  );
  return readModeStdioTapDevState();
}
