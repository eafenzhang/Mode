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
  create(params: {
    cols: number;
    rows: number;
    cwd?: string;
    /**
     * 稳定重挂接 key（仅壳-backed 实现消费；Host 实现忽略）。
     * 整页 reload 后用同一个 key 调 create，壳命中活会话时返回原 id 与有界回放。
     */
    sessionKey?: string;
  }): Promise<{
    id: string;
    shell: string;
    fontFamily: string;
    fontSize?: number;
    theme?: TerminalThemeProfile;
    fontFamilySource: TerminalFontFamilySource;
    windowsPty?: TerminalWindowsPtyInfo;
    /** 壳侧 reattach 命中的输出快照；新建会话为空。 */
    replay?: string;
    /** true = 命中已有 PTY，false/缺省 = 新建。 */
    reattached?: boolean;
  }>;
  write(params: { id: string; data: string }): Promise<void>;
  resize(params: { id: string; cols: number; rows: number }): Promise<void>;
  dispose(params: { id: string }): Promise<void>;
  /**
   * 按稳定 key 结束会话（**可选成员**，仅壳-backed 实现提供）。
   * 带 `sessionKey` 的终端在组件卸载时保活（Host 换代 fail-closed、workspace 切换），
   * 用户显式关闭 tab 时用本方法释放，避免 PTY 泄漏。
   */
  disposeByKey?(params: { sessionKey: string }): Promise<void>;
  onDynamicData(id: string): Event<string>;
  onDynamicExit(id: string): Event<number>;
}

export const ITerminalService = createServiceDescriptor<ITerminalService>(ServiceChannels.Terminal);
