import type { ElectronReleaseChannel, Locale } from "./protocol.js";

export interface PostUpdateReleaseNotesPayload {
  version: string;
  title: string;
  markdown: string;
  releaseDate?: string;
  releaseNotesByLocale?: Partial<Record<Locale, { title: string; markdown: string }>>;
}

/**
 * 用户从菜单手动点击"检查更新"后，main 进程回传给 renderer 的结果。
 * Renderer 根据 kind 展示对应的 toast；不要与启动时的自动 check 混用。
 */
export type UpdateCheckResultPayload =
  | { kind: "up-to-date"; currentVersion: string }
  | {
      kind: "available";
      version: string;
      channel?: ElectronReleaseChannel;
      releaseNotes?: PostUpdateReleaseNotesPayload;
    }
  | { kind: "downloading"; version: string }
  | { kind: "already-downloading"; version: string; progress: string }
  | { kind: "ready"; version: string }
  | { kind: "dev-skipped" }
  | { kind: "error"; message: string };

/**
 * 桌面自动更新器的持续状态，用于同步原生菜单和 Windows 自绘标题栏菜单。
 */
export type UpdateStatePayload =
  | { kind: "idle"; enabled: boolean }
  | { kind: "checking"; enabled: boolean }
  | {
      kind: "update-available";
      enabled: boolean;
      version: string;
      channel?: ElectronReleaseChannel;
      releaseNotes?: PostUpdateReleaseNotesPayload;
    }
  | {
      kind: "download-progress";
      enabled: boolean;
      progress: string;
      transferredBytes?: number;
      totalBytes?: number;
      version?: string;
      channel?: ElectronReleaseChannel;
      releaseNotes?: PostUpdateReleaseNotesPayload;
    }
  | {
      kind: "update-downloaded";
      enabled: boolean;
      version: string;
      channel?: ElectronReleaseChannel;
      releaseNotes?: PostUpdateReleaseNotesPayload;
    };

/**
 * fpk 在线升级（docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 与桌面自动更新状态机是两套所有者：桌面归 Tauri updater；这里归服务端
 * `FnosAppUpdater`，只通过 `/api/app-update/*` 投影给 Web UI。
 */
export type FnosAppUpdatePhase =
  | "idle"
  | "checking"
  | "up-to-date"
  | "available"
  | "downloading"
  | "ready"
  | "applying"
  | "error";

export interface FnosAppUpdateState {
  enabled: boolean;
  currentVersion: string;
  phase: FnosAppUpdatePhase;
  latestVersion?: string;
  /** 0..100；仅在 downloading 阶段有值。 */
  progress?: number;
  /** 稳定错误码或可读文本；仅在 error 阶段有值。 */
  error?: string;
  checkedAt?: string;
  canApply: boolean;
}

export interface FnosAppUpdateApplyResult {
  ok: boolean;
  restarting: boolean;
  error?: string;
}
