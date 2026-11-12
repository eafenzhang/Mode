import { WebNoiseHandshake, WebNoiseTransport } from "./noise.js";
import { buildWebPrologue } from "./prologue.js";
import type { WebIdentity } from "./identity.js";
import { withTimeout } from "./transport.js";

/** 执行 Noise IK 发起方握手；Control 帧的发送/等待由调用方注入。 */
export async function runInitiatorHandshake(params: {
  identity: WebIdentity;
  responderStaticKey: string;
  connectionId: string;
  hostDeviceId: string;
  clientDeviceId: string;
  timeoutMs: number;
  sendStep: (step: 1 | 2, data: Uint8Array) => void;
  waitStep2: () => Promise<Uint8Array>;
}): Promise<WebNoiseTransport> {
  const handshake = new WebNoiseHandshake({
    initiator: true,
    localIdentity: params.identity,
    responderStaticKey: params.responderStaticKey,
    prologue: buildWebPrologue({
      connectionId: params.connectionId,
      hostDeviceId: params.hostDeviceId,
      clientDeviceId: params.clientDeviceId,
    }),
  });
  params.sendStep(1, handshake.writeMessage());
  const step2 = await withTimeout(params.waitStep2(), params.timeoutMs, "等待 Noise 握手超时");
  handshake.readMessage(step2);
  return new WebNoiseTransport(handshake.split());
}
