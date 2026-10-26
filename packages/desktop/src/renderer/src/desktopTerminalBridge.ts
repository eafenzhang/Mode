/**
 * 终端 PTY 下沉到壳的渲染层桥（spec `terminal-pty-in-shell.md`）。
 *
 * shell-backed `ITerminalService`：profile 仍由 Host 提供（shell/字体/主题的权威），
 * 进程由壳拉起（`terminalCreate/Write/Resize/Dispose`，`apps/desktop-tauri/src-tauri/src/terminal.rs`），
 * 输出/退出经 Tauri event（`terminal://data` / `terminal://exit`）按 id 过滤。
 * Host 崩溃/重生不再带走终端——进程所有权在壳。
 *
 * 传输口径：invoke 与事件订阅都走 `window.__TAURI__`（withGlobalTauri），兜底
 * `__TAURI_INTERNALS__.invoke`；与注入桥（`crates/sidecar-host/src/window_mode.ts`）同一优先序。
 */
import { Emitter, type Event } from "@mode/rpc";
import type { ITerminalService, TerminalProfileInfo } from "@mode/services";

interface TauriGlobal {
  core?: { invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T> };
  event?: {
    listen: <T>(
      event: string,
      handler: (payload: { payload: T }) => void,
    ) => Promise<() => void>;
  };
}

function tauriGlobal(): TauriGlobal | undefined {
  return (window as { __TAURI__?: TauriGlobal }).__TAURI__;
}

function invokeViaTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const internals = (
    window as { __TAURI_INTERNALS__?: { invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T> } }
  ).__TAURI_INTERNALS__;
  const invoke = tauriGlobal()?.core?.invoke ?? internals?.invoke;
  if (!invoke) {
    return Promise.reject(new Error("tauri invoke unavailable"));
  }
  return invoke(command, args);
}

/** 订阅一次 Tauri event（幂等）；返回取消订阅的 Promise（丢弃即可，进程级存续）。 */
function listenViaTauri<T>(event: string, handler: (payload: T) => void): boolean {
  const listen = tauriGlobal()?.event?.listen;
  if (!listen) {
    return false;
  }
  void listen<T>(event, (message) => handler(message.payload));
  return true;
}

/** 壳-backed 终端服务。`profile` 由调用方接 Host（权威在 Host，见模块注释）。 */
export function createShellTerminalService(deps: {
  profile: (params: { cwd?: string }) => Promise<TerminalProfileInfo>;
}): ITerminalService {
  const dataEmitters = new Map<string, Emitter<string>>();
  const exitEmitters = new Map<string, Emitter<number>>();
  let listening = false;

  const ensureListeners = (): void => {
    if (listening) {
      return;
    }
    listening = true;
    listenViaTauri<{ id: string; data: string }>("terminal://data", (payload) => {
      dataEmitters.get(payload.id)?.fire(payload.data);
    });
    listenViaTauri<{ id: string; code: number }>("terminal://exit", (payload) => {
      exitEmitters.get(payload.id)?.fire(payload.code);
    });
  };

  return {
    async create(params: { cols: number; rows: number; cwd?: string }) {
      // profile 权威在 Host（shell/字体/主题/cwd 回退）；缺席 = 旧远端实现，如实报错。
      const profile = await deps.profile({ cwd: params.cwd });
      const { id } = await invokeViaTauri<{ id: string }>("terminalCreate", {
        cols: params.cols,
        rows: params.rows,
        cwd: profile.cwd,
        shell: profile.shell,
      });
      ensureListeners();
      return {
        id,
        shell: profile.shell,
        fontFamily: profile.fontFamily,
        fontSize: profile.fontSize,
        theme: profile.theme,
        fontFamilySource: profile.fontFamilySource,
        windowsPty: profile.windowsPty,
      };
    },

    async write(params: { id: string; data: string }): Promise<void> {
      await invokeViaTauri("terminalWrite", { id: params.id, data: params.data });
    },

    async resize(params: { id: string; cols: number; rows: number }): Promise<void> {
      await invokeViaTauri("terminalResize", {
        id: params.id,
        cols: params.cols,
        rows: params.rows,
      });
    },

    async dispose(params: { id: string }): Promise<void> {
      await invokeViaTauri("terminalDispose", { id: params.id });
      dataEmitters.delete(params.id);
      exitEmitters.delete(params.id);
    },

    onDynamicData(id: string): Event<string> {
      ensureListeners();
      let emitter = dataEmitters.get(id);
      if (!emitter) {
        emitter = new Emitter<string>();
        dataEmitters.set(id, emitter);
      }
      return emitter.event;
    },

    onDynamicExit(id: string): Event<number> {
      ensureListeners();
      let emitter = exitEmitters.get(id);
      if (!emitter) {
        emitter = new Emitter<number>();
        exitEmitters.set(id, emitter);
      }
      return emitter.event;
    },
  };
}
