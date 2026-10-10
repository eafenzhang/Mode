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
  const capabilities: RemoteWorkspaceSessionCapabilities | undefined =
    rawCapabilities && typeof rawCapabilities === "object"
      ? {
          ...(typeof (rawCapabilities as { computerUse?: unknown }).computerUse === "boolean"
            ? { computerUse: (rawCapabilities as { computerUse: boolean }).computerUse }
            : {}),
        }
      : undefined;
  return {
    attachmentId,
    port,
    sessionId,
    target,
    ...(capabilities ? { capabilities } : {}),
  };
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
