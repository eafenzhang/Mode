import {
  collectVisibleModeBackgroundTaskControlItems,
  getModeBackgroundTaskControlItemElapsedMs,
  isActiveModeBackgroundTaskControlItem,
  parseModeBackgroundTaskControlItems,
  type ModeBackgroundTaskControlItem,
  type ModeBackgroundTaskControlStatus,
} from "./background-task-controls.js";

export type ModeBackgroundBashJobStatus = ModeBackgroundTaskControlStatus;
export type ModeBackgroundBashJob = ModeBackgroundTaskControlItem & {
  taskKind: "bash";
};

export function parseModeBackgroundBashJobs(value: unknown): ModeBackgroundBashJob[] {
  return parseModeBackgroundTaskControlItems(value).filter(isBackgroundBashJob);
}

export function isActiveModeBackgroundBashJob(job: ModeBackgroundBashJob): boolean {
  return isActiveModeBackgroundTaskControlItem(job);
}

export function getModeBackgroundBashJobElapsedMs(
  job: ModeBackgroundBashJob,
  now = Date.now(),
): number {
  return getModeBackgroundTaskControlItemElapsedMs(job, now);
}

export function collectVisibleModeBackgroundBashJobs(
  jobs: readonly ModeBackgroundBashJob[],
  now = Date.now(),
  thresholdMs = 30_000,
): Array<ModeBackgroundBashJob & { elapsedMs: number }> {
  return collectVisibleModeBackgroundTaskControlItems(jobs, now, thresholdMs) as Array<
    ModeBackgroundBashJob & { elapsedMs: number }
  >;
}

function isBackgroundBashJob(job: ModeBackgroundTaskControlItem): job is ModeBackgroundBashJob {
  return job.taskKind === "bash";
}
