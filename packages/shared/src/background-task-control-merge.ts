import type { ModeBackgroundTaskControlItem } from "./background-task-controls.js";

export function mergeModeBackgroundTaskControlItems(
  current: readonly ModeBackgroundTaskControlItem[],
  updates: readonly ModeBackgroundTaskControlItem[],
): ModeBackgroundTaskControlItem[] {
  const jobsById = new Map(current.map((job) => [job.jobId, job] as const));
  for (const job of updates) {
    jobsById.set(job.jobId, job);
  }
  return Array.from(jobsById.values());
}
