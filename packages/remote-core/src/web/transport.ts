export interface WebSocketLike {
  readonly readyState: number;
  /** 发送队列水位（浏览器 WebSocket 原生提供）；媒体面据它做背压丢弃。 */
  readonly bufferedAmount?: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: string, listener: (event: { data?: unknown; code?: number }) => void): void;
}

type WebSocketCtor = new (url: string) => WebSocketLike;

export interface ControlEnvelope {
  v: 1;
  id: string;
  timestamp: number;
  type: string;
  payload: Record<string, unknown>;
}

/** 与 Node 版同口径：保留服务地址的路径前缀，只追加 Control 端点。 */
export function toControlUrl(serverUrl: string): string {
  const url = new URL(serverUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}/ws/v1/connect`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

export function openBrowserSocket(url: string): Promise<WebSocketLike> {
  const Ctor = (globalThis as { WebSocket?: WebSocketCtor }).WebSocket;
  if (!Ctor) throw new Error("当前环境没有 WebSocket");
  return new Promise((resolve, reject) => {
    const socket = new Ctor(url);
    socket.addEventListener("open", () => resolve(socket));
    socket.addEventListener("error", () => reject(new Error(`WebSocket 连接失败：${url}`)));
  });
}

export function parseControlEnvelope(text: string): ControlEnvelope | null {
  try {
    const value = JSON.parse(text) as Partial<ControlEnvelope>;
    if (value.v !== 1 || typeof value.type !== "string" || typeof value.id !== "string") {
      return null;
    }
    return {
      v: 1,
      id: value.id,
      timestamp: typeof value.timestamp === "number" ? value.timestamp : Date.now(),
      type: value.type,
      payload:
        value.payload && typeof value.payload === "object"
          ? (value.payload as Record<string, unknown>)
          : {},
    };
  } catch {
    return null;
  }
}

export function requireString(value: unknown, label: string): string {
  if (typeof value !== "string" || !value) throw new Error(`${label} 缺失`);
  return value;
}

export function randomId(): string {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi && typeof cryptoApi.randomUUID === "function") return cryptoApi.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
