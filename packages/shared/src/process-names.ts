const MODE_PROCESS_PREFIX = "mode";
const MAX_PROCESS_NAME_SEGMENT_LENGTH = 24;

function sanitizeProcessNameSegment(value: string | null | undefined): string | null {
  if (!value) {
    return null;
  }

  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!normalized) {
    return null;
  }

  return normalized.slice(0, MAX_PROCESS_NAME_SEGMENT_LENGTH);
}

function joinModeProcessName(...segments: Array<string | null | undefined>): string {
  const sanitizedSegments = segments
    .map((segment) => sanitizeProcessNameSegment(segment))
    .filter((segment): segment is string => Boolean(segment));
  return [MODE_PROCESS_PREFIX, ...sanitizedSegments].join("-");
}

function pickWorkspaceTag(workspacePath: string | null | undefined): string | undefined {
  const trimmedPath = workspacePath?.trim();
  if (!trimmedPath) {
    return undefined;
  }

  const parts = trimmedPath.split(/[\\/]+/).filter(Boolean);
  return parts.at(-1) ?? trimmedPath;
}

export function formatModeMainProcessName(): string {
  return joinModeProcessName("main");
}

export function formatModeGpuProcessName(): string {
  return joinModeProcessName("gpu");
}

export function formatModeHostProcessName(label?: string): string {
  return joinModeProcessName("host", label);
}

export function formatModeRendererProcessName(windowTitle?: string): string {
  const normalizedTitle = windowTitle?.trim();
  if (!normalizedTitle || normalizedTitle === "Mode") {
    return joinModeProcessName("renderer", "main");
  }

  if (normalizedTitle === "Resource Manager") {
    return joinModeProcessName("renderer", "resource-manager");
  }

  const remoteWindowPrefix = "Mode - ";
  if (normalizedTitle.startsWith(remoteWindowPrefix)) {
    return joinModeProcessName(
      "renderer",
      "remote",
      normalizedTitle.slice(remoteWindowPrefix.length),
    );
  }

  return joinModeProcessName("renderer", normalizedTitle);
}

export function formatModeAgentProcessName(provider: string, workspacePath?: string): string {
  return joinModeProcessName("agent", provider, pickWorkspaceTag(workspacePath));
}

export function formatModeUtilityProcessName(name?: string, type = "utility"): string {
  return joinModeProcessName(type, name);
}
