/** remote-core 对外契约：设备身份、协议帧、安全会话与流多路复用。 */
export type {
  BusinessFrame,
  EventFrame,
  PingFrame,
  PongFrame,
  RpcErrorFrame,
  RpcRequestFrame,
  RpcResponseFrame,
  StreamCloseFrame,
  StreamDataFrame,
  StreamErrorFrame,
  StreamOpenFrame,
} from "./domain/businessFrames.js";
export {
  decodeBusinessFrame,
  encodeBusinessFrame,
} from "./domain/businessFrames.js";
export type {
  ConnectAcceptedPayload,
  ConnectIncomingPayload,
  ConnectRequestPayload,
  ControlFrame,
  DirectAssignment,
  ErrorPayload,
  HelloAckPayload,
  HelloPayload,
  RelayAssignment,
  RelayAttachPayload,
  RelayPayload,
  SecureHandshakePayload,
  TransportKind,
  TransportSelectedPayload,
} from "./domain/controlFrames.js";
export { decodeControlFrame, encodeControlFrame } from "./domain/controlFrames.js";
export type { ConnectionContext } from "./domain/prologue.js";
export type { DeviceDescriptor, DeviceRole } from "./domain/descriptor.js";
export {
  identityFingerprint,
  isDeviceId,
  isValidIdentityKey,
  parseDeviceDescriptor,
} from "./domain/descriptor.js";
export { RemoteError } from "./domain/errors.js";
export type { IdentityKeyPair } from "./adapters/identity.js";
export {
  exportIdentityPrivateKey,
  generateIdentity,
  identityPublicKeyRaw,
  importIdentityFromPkcs8,
  publicKeyObject,
} from "./adapters/identity.js";
export type { NoiseHandshakeOptions, NoiseTransportPair } from "./adapters/noise.js";
export { NoiseHandshake, NoiseTransport } from "./adapters/noise.js";
export type { SecureSessionOptions, SecureSessionState } from "./app/secureSession.js";
export { SecureSession } from "./app/secureSession.js";
export type { RemoteStream, StreamMuxOptions } from "./app/streamMux.js";
export { StreamMux } from "./app/streamMux.js";
export type {
  ControlLinkOptions,
  RemoteConnection,
  RemoteSocket,
} from "./app/controlTypes.js";
export { ControlLink, toControlUrl } from "./app/controlLink.js";
