import type { WebControlLink, WebRemoteConnection } from "@mode/remote-core/web";
import type {
  RemoteV3HttpRequestParams,
  RemoteV3HttpResponse,
  RemoteV3RequestParams,
  RemoteV3RequestResult,
} from "@mode/shared";

/** `request/httpRequest` 只需要连接上的业务帧与 HTTP 桥，不关心连接如何建立。 */
export interface WebRequestConnection {
  link: Pick<WebControlLink, "request">;
  connection: WebRemoteConnection;
}

/** 浏览器 Client 的 allowlist RPC 与 Host 本机 `/api/*` 桥接。 */
export function createWebRequestMethods(
  ensureConnection: (hostDeviceId: string) => Promise<WebRequestConnection>,
) {
  return {
    async request(params: RemoteV3RequestParams): Promise<RemoteV3RequestResult> {
      const attachment = await ensureConnection(params.hostDeviceId);
      const id = crypto.randomUUID();
      const timeoutMs = params.timeoutMs ?? 15_000;
      return await new Promise<RemoteV3RequestResult>((resolve, reject) => {
        const timer = setTimeout(() => {
          dispose();
          reject(new Error(`remote.request 超时：${params.method}`));
        }, timeoutMs);
        const dispose = attachment.connection.onFrame((frame) => {
          if (frame.type !== "rpc.response" && frame.type !== "rpc.error") return;
          if (frame.id !== id) return;
          if (frame.type === "rpc.response") {
            clearTimeout(timer);
            dispose();
            resolve({ result: frame.result });
            return;
          }
          if (frame.type === "rpc.error") {
            clearTimeout(timer);
            dispose();
            reject(new Error(`${frame.code}: ${frame.message}`));
          }
        });
        attachment.connection.send({
          v: 1,
          type: "rpc.request",
          id,
          method: params.method,
          params: params.params ?? {},
        });
      });
    },
    async httpRequest(params: RemoteV3HttpRequestParams): Promise<RemoteV3HttpResponse> {
      const attachment = await ensureConnection(params.hostDeviceId);
      return (await attachment.link.request(
        "remote.http.request",
        {
          method: params.method,
          path: params.path,
          ...(params.headers ? { headers: params.headers } : {}),
          ...(params.bodyBase64 ? { bodyBase64: params.bodyBase64 } : {}),
          ...(params.timeoutMs ? { timeoutMs: params.timeoutMs } : {}),
        },
        params.timeoutMs ?? 15_000,
      )) as RemoteV3HttpResponse;
    },
  };
}
