import type { Event, IDisposable } from "@zcode/rpc";
import type { ModeProtocolMessage } from "@zcode/shared";

export type ZCodeProtocolTransportKind = "stdio" | "websocket" | "memory";

export interface ZCodeProtocolTransportClosedEvent {
  code?: number | null;
  signal?: NodeJS.Signals | null;
  reason?: string;
}

export interface ZCodeProtocolTransport extends IDisposable {
  readonly kind: ZCodeProtocolTransportKind;
  readonly onMessage: Event<ModeProtocolMessage>;
  readonly onClose: Event<ZCodeProtocolTransportClosedEvent>;
  send(message: ModeProtocolMessage): Promise<void>;
  disposeAndWait?(): Promise<void>;
}
