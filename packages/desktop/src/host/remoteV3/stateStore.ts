import type { RemoteV3HostState } from "@mode/shared";

const INITIAL: RemoteV3HostState = {
  status: "unconfigured",
  serverUrl: null,
  accountName: null,
  deviceId: null,
  deviceName: null,
  fingerprint: null,
  error: null,
  activeConnections: 0,
  pendingLogin: null,
};

/** Host 管理面的唯一状态持有者；renderer 只订阅快照。 */
export class RemoteV3HostStateStore {
  private state: RemoteV3HostState = { ...INITIAL };
  private readonly listeners = new Set<(state: RemoteV3HostState) => void>();

  get snapshot(): RemoteV3HostState {
    return {
      ...this.state,
      pendingLogin: this.state.pendingLogin ? { ...this.state.pendingLogin } : null,
    };
  }

  setInitial(patch: Partial<RemoteV3HostState>): void {
    this.state = { ...INITIAL, ...patch };
  }

  patch(patch: Partial<RemoteV3HostState>): void {
    this.state = { ...this.state, ...patch };
    const snapshot = this.snapshot;
    for (const listener of this.listeners) listener(snapshot);
  }

  incrementConnections(): void {
    this.patch({ activeConnections: this.state.activeConnections + 1 });
  }

  decrementConnections(): void {
    this.patch({ activeConnections: Math.max(0, this.state.activeConnections - 1) });
  }

  readonly onDidChange = (listener: (state: RemoteV3HostState) => void) => {
    this.listeners.add(listener);
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  };
}
