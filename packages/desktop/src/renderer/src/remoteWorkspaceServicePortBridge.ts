import { InternalChannels, type RemoteTarget } from "@mode/shared";

export interface RemoteWorkspaceSessionCapabilities {
  computerUse?: boolean;
}

export interface RemoteWorkspaceServicePortRegistration {
  attachmentId: string;
  port: MessagePort;
  sessionId: string;
  target: RemoteTarget;
  capabilities?: RemoteWorkspaceSessionCapabilities;
}

export function parseRemoteWorkspaceServicePortMessage(
  event: MessageEvent,
): RemoteWorkspaceServicePortRegistration | null {
  if (
    typeof event.data !== "object" ||
    event.data === null ||
    !("type" in event.data) ||
    event.data.type !== InternalChannels.ScopedServicePort
  ) {
    return null;
  }
  const port = event.ports[0];
  const attachmentId = typeof event.data.attachmentId === "string" ? event.data.attachmentId : null;
  const sessionId = typeof event.data.sessionId === "string" ? event.data.sessionId : null;
  const target = "target" in event.data ? (event.data.target as RemoteTarget | null) : null;
  if (!port || !attachmentId || !sessionId || !target) return null;
  const rawCapabilities = "capabilities" in event.data ? event.data.capabilities : null;
  // 语义保持不变：capabilities 缺席/非对象 → 注册信息不带该键；对象在场 → 键在场，
  // computerUse 仅布尔值时成键（旧对端缺键按不支持降级）。
  const registration: RemoteWorkspaceServicePortRegistration = {
    attachmentId,
    port,
    sessionId,
    target,
  };
  if (rawCapabilities && typeof rawCapabilities === "object") {
    const computerUse = (rawCapabilities as { computerUse?: unknown }).computerUse;
    registration.capabilities = typeof computerUse === "boolean" ? { computerUse } : {};
  }
  return registration;
}

export function notifyRemoteWorkspaceServicePortReady(
  payload: Pick<RemoteWorkspaceServicePortRegistration, "attachmentId" | "sessionId">,
  postMessage: (message: unknown, targetOrigin: string) => void = window.postMessage.bind(window),
): void {
  postMessage(
    {
      type: InternalChannels.ScopedServicePortReady,
      attachmentId: payload.attachmentId,
      sessionId: payload.sessionId,
    },
    "*",
  );
}
