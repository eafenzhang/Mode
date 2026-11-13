import { StreamMux, type RemoteConnection, type RemoteStream } from "@mode/remote-core";

import { handleRemoteV3Frame, type RemoteV3RpcContext } from "./rpc.js";
import { attachLocalWebSocketTunnel, type LocalApiTarget } from "./wsTunnel.js";

export interface HostConnectionDeps {
  context: RemoteV3RpcContext;
  getLocalApi?: () => Promise<LocalApiTarget | null>;
  log: (line: string) => void;
  onOpen: () => void;
  onClose: () => void;
}

/** 把一条已认证的 Noise 连接接到 Host 业务 allowlist 与 `/ws*` 隧道。 */
export function attachHostConnection(
  connection: RemoteConnection,
  deps: HostConnectionDeps,
): void {
  deps.onOpen();
  deps.log(
    `remote_v3_connection_open peer=${connection.peer.deviceId} fingerprint=${connection.peer.identityKey.slice(0, 8)}`,
  );
  const mux = new StreamMux({
    send: (frame) => connection.send(frame),
    onStream: (stream) => void handleIncomingStream(stream, deps),
    getBufferedAmount: () => connection.bufferedAmount,
  });
  connection.onFrame((frame) => {
    if (mux.handle(frame)) return;
    void handleRemoteV3Frame(connection, frame, deps.context).catch((error: unknown) => {
      deps.log(
        `remote_v3_rpc_failed peer=${connection.peer.deviceId} error=${error instanceof Error ? error.message : String(error)}`,
      );
    });
  });
  connection.onClose(() => {
    mux.dispose();
    deps.onClose();
    deps.log(`remote_v3_connection_closed peer=${connection.peer.deviceId}`);
  });
}

async function handleIncomingStream(stream: RemoteStream, deps: HostConnectionDeps): Promise<void> {
  if (stream.service !== "remote.ws") {
    stream.close("unsupported stream service");
    return;
  }
  try {
    const target = await deps.getLocalApi?.();
    if (!target) {
      stream.close("local api unavailable");
      return;
    }
    attachLocalWebSocketTunnel(stream, target);
  } catch (error) {
    stream.close(error instanceof Error ? error.message : String(error));
  }
}
