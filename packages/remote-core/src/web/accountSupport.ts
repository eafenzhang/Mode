interface WebProfileShape {
  serverUrl: string;
  deviceId: string;
  deviceName: string;
  privateKey: string;
  accessToken: string | null;
  refreshToken: string | null;
}

export function parseWebProfile(raw: string): WebProfileShape | null {
  try {
    const value = JSON.parse(raw) as Partial<WebProfileShape>;
    if (
      typeof value.serverUrl !== "string" ||
      typeof value.deviceId !== "string" ||
      typeof value.deviceName !== "string" ||
      typeof value.privateKey !== "string"
    ) {
      return null;
    }
    return {
      serverUrl: value.serverUrl,
      deviceId: value.deviceId,
      deviceName: value.deviceName,
      privateKey: value.privateKey,
      accessToken: typeof value.accessToken === "string" ? value.accessToken : null,
      refreshToken: typeof value.refreshToken === "string" ? value.refreshToken : null,
    };
  } catch {
    return null;
  }
}

export async function readWebJson(response: Response): Promise<Record<string, unknown>> {
  try {
    const value = (await response.json()) as unknown;
    return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function readWebError(body: Record<string, unknown>, fallback: string): string {
  const error = body.error as { message?: unknown } | undefined;
  return typeof error?.message === "string" && error.message ? error.message : fallback;
}

export function webRandomUUID(): string {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi && typeof cryptoApi.randomUUID === "function") return cryptoApi.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}
