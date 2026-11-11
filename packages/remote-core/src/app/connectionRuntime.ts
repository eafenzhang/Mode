import { decodeBase64Url, encodeBase64Url } from "../domain/base64url.js";
import type { BusinessFrame } from "../domain/businessFrames.js";
import {
  decodeControlFrame,
  type ControlFrame,
  type RelayAssignment,
} from "../domain/controlFrames.js";
import type { DeviceDescriptor } from "../domain/descriptor.js";
import type { RemoteError } from "../domain/errors.js";
import type { IdentityKeyPair } from "../adapters/identity.js";
import { SecureSession } from "./secureSession.js";
import type { RemoteConnection, RemoteSocket } from "./controlTypes.js";

export interface ActiveSession {
  connectionId: string;
  peer: DeviceDescriptor;
  session: SecureSession;
  relay: RelayAssignment;
  transportSelected: boolean;
  dataSocket: RemoteSocket | null;
  listeners: Set<(frame: BusinessFrame) => void>;
  ready: Promise<RemoteConnection>;
  resolveReady: (connection: RemoteConnection) => void;
  rejectReady: (error: RemoteError) => void;
}

export interface ConnectionRuntimeDeps {
  identity: IdentityKeyPair;
  deviceId: string;
  connectSocket: (url: string) => Promise<RemoteSocket>;
  sendControl: (frame: { type: ControlFrame["type"]; payload: unknown }) => void;
  sendOn: (socket: RemoteSocket, frame: { type: ControlFrame["type"]; payload: unknown }) => void;
  onError?: (error: RemoteError) => void;
}

/** 按 connectionId 隔离的 Noise/Relay 会话；Host 与 Client 共用。 */
export class ConnectionRuntime {
  readonly sessions = new Map<string, ActiveSession>();
  private readonly pendingHandshakes = new Map<
    string,
    Array<{ step: 1 | 2; data: Uint8Array }>
  >();
  private readonly deps: ConnectionRuntimeDeps;

  constructor(deps: ConnectionRuntimeDeps) {
    this.deps = deps;
  }

  queueHandshake(connectionId: string, step: 1 | 2, data: Uint8Array): void {
    const active = this.sessions.get(connectionId);
    if (active) {
      active.session.handleHandshake(step, data);
      return;
    }
    const pending = this.pendingHandshakes.get(connectionId) ?? [];
    if (pending.length < 2) pending.push({ step, data });
    this.pendingHandshakes.set(connectionId, pending);
  }

  create(role: "host" | "client", peer: DeviceDescriptor, connectionId: string): SecureSession {
    const listeners = new Set<(frame: BusinessFrame) => void>();
    const closeListeners = new Set<() => void>();
    const earlyFrames: BusinessFrame[] = [];
    let resolveReady: (value: RemoteConnection) => void = () => undefined;
    let rejectReady: (error: RemoteError) => void = () => undefined;
    const ready = new Promise<RemoteConnection>((resolve, reject) => {
      resolveReady = resolve;
      rejectReady = reject;
    });
    const connection: RemoteConnection = {
      connectionId,
      peer,
      role,
      send: (frame) => session.send(frame),
      onFrame: (callback) => {
        listeners.add(callback);
        for (const frame of earlyFrames.splice(0, earlyFrames.length)) callback(frame);
        return () => listeners.delete(callback);
      },
      onClose: (callback) => {
        closeListeners.add(callback);
        return () => closeListeners.delete(callback);
      },
      close: (reason) => {
        session.close(reason ?? "closed");
        this.sessions.delete(connectionId);
      },
    };
    const session = new SecureSession({
      role,
      localIdentity: this.deps.identity,
      peerIdentityKey: peer.identityKey,
      context: {
        connectionId,
        hostDeviceId: role === "host" ? this.deps.deviceId : peer.deviceId,
        clientDeviceId: role === "client" ? this.deps.deviceId : peer.deviceId,
      },
      sendHandshake: (step, data) => {
        this.deps.sendControl({
          type: "secure.handshake",
          payload: {
            connectionId,
            targetDeviceId: peer.deviceId,
            step,
            data: encodeBase64Url(data),
          },
        });
      },
      onFrame: (frame) => {
        if (listeners.size === 0) {
          if (earlyFrames.length < 32) earlyFrames.push(frame);
          return;
        }
        for (const listener of listeners) listener(frame);
      },
      onReady: () => resolveReady(connection),
      onError: (error) => {
        rejectReady(error);
        this.deps.onError?.(error);
      },
      onClose: () => {
        active.dataSocket?.close(1000, "session closed");
        for (const listener of closeListeners) listener();
        closeListeners.clear();
        this.sessions.delete(connectionId);
      },
    });
    const active: ActiveSession = {
      connectionId,
      peer,
      session,
      relay: null as unknown as RelayAssignment,
      transportSelected: role === "host",
      dataSocket: null,
      listeners,
      ready,
      resolveReady,
      rejectReady,
    };
    this.sessions.set(connectionId, active);
    const pending = this.pendingHandshakes.get(connectionId);
    if (pending) {
      this.pendingHandshakes.delete(connectionId);
      for (const message of pending) session.handleHandshake(message.step, message.data);
    }
    return session;
  }

  async attachRelay(
    active: ActiveSession,
    relay: RelayAssignment,
    timeoutMs = 10_000,
  ): Promise<void> {
    active.relay = relay;
    const socket = await this.deps.connectSocket(relay.endpoint);
    active.dataSocket = socket;
    let readyResolve: () => void = () => undefined;
    let readyReject: (error: Error) => void = () => undefined;
    const ready = new Promise<void>((resolve, reject) => {
      readyResolve = resolve;
      readyReject = reject;
    });
    socket.onMessage((text) => {
      let frame: ControlFrame;
      try {
        frame = decodeControlFrame(Buffer.from(text, "utf8"));
      } catch {
        return;
      }
      if (frame.type === "relay.ready") {
        readyResolve();
        return;
      }
      if (frame.type === "relay.detach") {
        active.session.close("relay detached");
        return;
      }
      if (frame.type === "relay") {
        active.session.handleData(decodeBase64Url(frame.payload.ciphertext, "relay.ciphertext"));
      }
    });
    socket.onClose(() => {
      readyReject(new Error("relay socket closed"));
      active.session.close("relay socket closed");
    });
    this.deps.sendOn(socket, {
      type: "relay.attach",
      payload: {
        connectionId: active.connectionId,
        deviceId: this.deps.deviceId,
        peerDeviceId: active.peer.deviceId,
        ticket: relay.ticket,
      },
    });
    const timeout = setTimeout(() => readyReject(new Error("relay.attach timeout")), timeoutMs);
    try {
      await ready;
    } finally {
      clearTimeout(timeout);
    }
    let counter = 0;
    active.session.attachData((bytes) => {
      this.deps.sendOn(socket, {
        type: "relay",
        payload: {
          connectionId: active.connectionId,
          targetDeviceId: active.peer.deviceId,
          counter: counter++,
          ciphertext: encodeBase64Url(bytes),
        },
      });
    });
  }

  /**
   * LAN 直连数据面：socket 由 Host 的 `/ws/v1/direct` 或客户端直连适配器完成
   * ticket 校验（`relay.ready` 已在适配器内消费）；这里只负责 Noise 密文收发。
   */
  attachDirect(active: ActiveSession, socket: RemoteSocket): void {
    if (active.dataSocket) {
      socket.close(1000, "direct socket already attached");
      return;
    }
    active.dataSocket = socket;
    socket.onMessage((text) => {
      let frame: ControlFrame;
      try {
        frame = decodeControlFrame(Buffer.from(text, "utf8"));
      } catch {
        return;
      }
      if (frame.type === "relay") {
        active.session.handleData(decodeBase64Url(frame.payload.ciphertext, "relay.ciphertext"));
        return;
      }
      if (frame.type === "relay.detach") {
        active.session.close("direct detached");
      }
    });
    socket.onClose(() => active.session.close("direct socket closed"));
    let counter = 0;
    active.session.attachData((bytes) => {
      this.deps.sendOn(socket, {
        type: "relay",
        payload: {
          connectionId: active.connectionId,
          targetDeviceId: active.peer.deviceId,
          counter: counter++,
          ciphertext: encodeBase64Url(bytes),
        },
      });
    });
  }

  closeAll(reason: string): void {
    for (const active of this.sessions.values()) {
      active.session.close(reason);
      active.dataSocket?.close(1000, reason);
    }
    this.sessions.clear();
    this.pendingHandshakes.clear();
  }
}
