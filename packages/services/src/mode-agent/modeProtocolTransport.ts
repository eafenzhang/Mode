import type { Event, IDisposable } from "@mode/rpc";
import type { ModeProtocolMessage } from "@mode/shared";

export type ModeProtocolTransportKind = "stdio" | "websocket" | "memory";

export interface ModeProtocolTransportClosedEvent {
  code?: number | null;
  signal?: NodeJS.Signals | null;
  reason?: string;
}

export interface ModeProtocolTransport extends IDisposable {
  readonly kind: ModeProtocolTransportKind;
  readonly onMessage: Event<ModeProtocolMessage>;
  readonly onClose: Event<ModeProtocolTransportClosedEvent>;
  send(message: ModeProtocolMessage): Promise<void>;
  disposeAndWait?(): Promise<void>;
}
