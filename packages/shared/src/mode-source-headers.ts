import { DEFAULT_MODE_ENDPOINT_ORIGIN } from "./modeEndpoint.js";

export const MODE_SOURCE_HEADERS = {
  "User-Agent": "Mode/unknown",
  "HTTP-Referer": DEFAULT_MODE_ENDPOINT_ORIGIN,
  "X-Title": "Z Code@electron",
} as const;

export interface BuildModeSourceHeadersFromContextOptions {
  appVersion?: string;
  arch?: string;
  clientLanguage?: string;
  clientTimezone?: string;
  deviceMid?: string;
  endpointOrigin?: string;
  osVersion?: string;
  platform?: string;
  releaseChannel?: string;
  sourceTitle?: string;
}

export function normalizeModeSourceHeaderValue(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed || !/^[\x20-\x7e]+$/.test(trimmed)) {
    return undefined;
  }
  return trimmed;
}

export function buildModeSourceHeadersFromContext(
  options: BuildModeSourceHeadersFromContextOptions = {},
): Record<string, string> {
  const appVersion = normalizeModeSourceHeaderValue(options.appVersion);
  const arch = normalizeModeSourceHeaderValue(options.arch);
  const clientLanguage = normalizeModeSourceHeaderValue(options.clientLanguage) ?? "unknown";
  const clientTimezone = normalizeModeSourceHeaderValue(options.clientTimezone) ?? "unknown";
  const deviceMid = normalizeModeSourceHeaderValue(options.deviceMid);
  const endpointOrigin =
    normalizeModeSourceHeaderValue(options.endpointOrigin) ?? DEFAULT_MODE_ENDPOINT_ORIGIN;
  const osVersion = normalizeModeSourceHeaderValue(options.osVersion);
  const platform = normalizeModeSourceHeaderValue(options.platform);
  const releaseChannel = normalizeModeSourceHeaderValue(options.releaseChannel);
  const sourceTitle = normalizeModeSourceHeaderValue(options.sourceTitle) ?? "electron";

  return {
    ...MODE_SOURCE_HEADERS,
    "HTTP-Referer": endpointOrigin,
    "User-Agent": `Mode/${appVersion ?? "unknown"}`,
    ...(appVersion ? { "X-Mode-App-Version": appVersion } : {}),
    "X-Title": `Z Code@${sourceTitle}`,
    ...(platform && arch ? { "X-Platform": `${platform}-${arch}` } : {}),
    ...(releaseChannel ? { "X-Release-Channel": releaseChannel } : {}),
    "X-Client-Language": clientLanguage,
    "X-Client-Timezone": clientTimezone,
    ...(platform ? { "X-Os-Category": normalizeOsCategory(platform) } : {}),
    ...(osVersion ? { "X-Os-Version": osVersion } : {}),
    ...(deviceMid ? { "X-Device-Mid": deviceMid } : {}),
  };
}

function normalizeOsCategory(platform: string): string {
  switch (platform) {
    case "darwin":
      return "macos";
    case "win32":
      return "windows";
    default:
      return "linux";
  }
}
