/** remote-server 对外契约：服务启动、账号/设备存储、网关与票据。 */
export { startRemoteServer, type RemoteServerHandle, type RemoteServerOptions } from "./server.js";
export { CloudStore, StoreError, type ConnectionRow, type DeviceRow } from "./store.js";
export { createGithubVerifier, type GitHubIdentity, type GitHubVerifier } from "./github.js";
export { RemoteGateway, GatewayError, descriptorOf } from "./gateway.js";
export {
  createDirectTicket,
  createRelayTicket,
  verifyDirectTicket,
  verifyRelayTicket,
  type DirectTicketPayload,
  type RelayTicketPayload,
} from "./tickets.js";
