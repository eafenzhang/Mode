import type { ControlFrame } from "../domain/controlFrames.js";
import type {
  ConnectAcceptedPayload,
  ConnectIncomingPayload,
  ErrorPayload,
  P2PCandidatePayload,
  P2POfferPayload,
  SecureHandshakePayload,
  TransportSelectedPayload,
} from "../domain/controlMessages.js";

export interface ControlFrameHandlers {
  onConnectAccepted(payload: ConnectAcceptedPayload): void;
  onConnectIncoming(payload: ConnectIncomingPayload): void;
  onTransportSelected(payload: TransportSelectedPayload): void;
  onSecureHandshake(payload: SecureHandshakePayload): void;
  onRelayDetach(connectionId: string): void;
  onError(payload: ErrorPayload): void;
  onP2POffer(payload: P2POfferPayload): void;
  onP2PCandidate(payload: P2PCandidatePayload): void;
  onPing(nonce?: string): void;
  onPong(): void;
}

/**
 * Control 帧到链路动作的纯分发：只做类型判别，不持有状态。
 *
 * 未列出的帧（hello、connect.request、relay 各帧、p2p.answer 等）由 waiter 或运行时处理，
 * 这里静默忽略，避免链路层重复解释同一条帧。
 */
export function dispatchControlFrame(
  frame: ControlFrame,
  handlers: ControlFrameHandlers,
): void {
  switch (frame.type) {
    case "hello.ack":
      return;
    case "connect.accepted":
      handlers.onConnectAccepted(frame.payload);
      return;
    case "connect.incoming":
      handlers.onConnectIncoming(frame.payload);
      return;
    case "transport.selected":
      handlers.onTransportSelected(frame.payload);
      return;
    case "secure.handshake":
      handlers.onSecureHandshake(frame.payload);
      return;
    case "relay.detach":
      handlers.onRelayDetach(frame.payload.connectionId);
      return;
    case "error":
      handlers.onError(frame.payload);
      return;
    case "p2p.offer":
      handlers.onP2POffer(frame.payload);
      return;
    case "p2p.candidate":
      handlers.onP2PCandidate(frame.payload);
      return;
    case "ping":
      handlers.onPing(frame.payload.nonce);
      return;
    case "pong":
      handlers.onPong();
      return;
    default:
      return;
  }
}
