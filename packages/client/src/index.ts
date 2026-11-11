export { RemoteServiceAccess } from "./remoteServiceAccess.js";
export {
  connectViaProtocol,
  connectViaProtocolWithClient,
  connectViaWebSocket,
} from "./websocket.js";
export type { WebSocketConnectionCloseEvent } from "./websocket.js";
export { connectViaMessagePort, createMessagePortServiceConnection } from "./messageport.js";
export type { MessagePortServiceConnection } from "./messageport.js";
export { openWebWorkspace, type WebWorkspaceConnection } from "./webWorkspace.js";
export {
  createWebRemoteDesktopSession,
  type CreateWebRemoteDesktopSessionParams,
  type WebRemoteDesktopSession,
} from "./remoteDesktop/web.js";
export {
  createRemoteDesktopHostController,
  type CreateRemoteDesktopHostControllerOptions,
  type RemoteDesktopHostController,
  type RemoteDesktopPeerSessionFailure,
  type RemoteDesktopPeerSessionOpen,
  type RemoteDesktopSignalSocket,
} from "./remoteDesktop/controller.js";
