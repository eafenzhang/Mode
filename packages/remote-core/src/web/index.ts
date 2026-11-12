/** 浏览器可用的远程控制 v3 密码学原语；不依赖 node:crypto / Buffer。 */
export { decodeBase64Url, encodeBase64Url } from "./base64url.js";
export {
  exportWebIdentity,
  generateWebIdentity,
  importWebIdentity,
  WEB_IDENTITY_KEY_BYTES,
  type WebIdentity,
} from "./identity.js";
export {
  WebNoiseHandshake,
  WebNoiseTransport,
  type WebNoiseHandshakeOptions,
  type WebNoiseTransportPair,
} from "./noise.js";
export { buildWebPrologue, type WebConnectionContext } from "./prologue.js";
export {
  WebControlLink,
  type WebControlLinkOptions,
  type WebRemoteConnection,
} from "./controlLink.js";
export { type WebBusinessStream } from "./streams.js";
export { createEncryptedCredentialStore } from "./secureStore.js";
export {
  WebAccountClient,
  WebApiError,
  exchangeGithubAccessToken,
  type WebCredentialStore,
  type WebDeviceProfile,
  type WebDeviceSummary,
  type WebGithubChallenge,
  type WebGithubPoll,
} from "./account.js";
export {
  clearWebAuth,
  deleteWebProfile,
  listWebProfiles,
  loadOrCreateWebProfile,
  normalizeWebServerUrl,
  saveWebProfile,
  webProfileKey,
  type WebAccountSummary,
} from "./accountSupport.js";
