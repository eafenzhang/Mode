import type { Event } from "@mode/rpc";
import { ServiceChannels } from "@mode/shared";
import { createServiceDescriptor } from "../descriptors.js";
import type { TerminalFontFamilySource, TerminalThemeProfile } from "./terminalProfile.js";

export interface TerminalWindowsPtyInfo {
  backend: "conpty" | "winpty";
  buildNumber?: number;
}

/** 终端 profile：shell/cwd 已由 Host 解析（含失效回退），渲染层把它交给壳的 PTY 拉起。 */
export interface TerminalProfileInfo {
  shell: string;
  cwd: string;
  fontFamily: string;
  fontSize?: number;
  theme?: TerminalThemeProfile;
  fontFamilySource: TerminalFontFamilySource;
  windowsPty?: TerminalWindowsPtyInfo;
}

export interface ITerminalService {
  /**
   * 终端 profile（**可选成员**，2026-10-26）：桌面终端 PTY 下沉到壳
   * （spec `terminal-pty-in-shell.md`）后，壳-backed 实现用它取 shell 与字体/主题，
   * 进程由壳拉起。Host 实现提供；缺该成员的实现（旧远端）由调用方回退原路径。
   */
  profile?(params: { cwd?: string }): Promise<TerminalProfileInfo>;
  create(params: { cols: number; rows: number; cwd?: string }): Promise<{
    id: string;
    shell: string;
    fontFamily: string;
    fontSize?: number;
    theme?: TerminalThemeProfile;
    fontFamilySource: TerminalFontFamilySource;
    windowsPty?: TerminalWindowsPtyInfo;
  }>;
  write(params: { id: string; data: string }): Promise<void>;
  resize(params: { id: string; cols: number; rows: number }): Promise<void>;
  dispose(params: { id: string }): Promise<void>;
  onDynamicData(id: string): Event<string>;
  onDynamicExit(id: string): Event<number>;
}

export const ITerminalService = createServiceDescriptor<ITerminalService>(ServiceChannels.Terminal);
