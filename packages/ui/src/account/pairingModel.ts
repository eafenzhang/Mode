/**
 * 跨账号配对面板的纯状态机（docs/specs/remote-control-v3-pairing.md）。
 *
 * 只描述 UI 阶段与用户输入；网络调用与轮询在 `RemoteV3PairingPanel`。
 * Host 侧：ticket → 等对方 claim → 输入验证码 → done。
 * Client 侧：claim → 展示验证码 → 等 Host 确认并 redeem → done。
 */
export type RemoteV3PairingUiState =
  | { kind: "idle" }
  | {
      kind: "host";
      pairingId: string;
      ticket: string;
      expiresAt: number;
      claimed: boolean;
      code: string;
    }
  | { kind: "host-done" }
  | {
      kind: "client";
      pairingId: string;
      verificationCode: string;
      hostName: string;
      expiresAt: number;
    }
  | { kind: "client-done" }
  | { kind: "error"; message: string };

export type RemoteV3PairingUiAction =
  | { type: "host-started"; pairingId: string; ticket: string; expiresAt: number }
  | { type: "host-claimed" }
  | { type: "host-code"; code: string }
  | { type: "host-done" }
  | {
      type: "client-claimed";
      pairingId: string;
      verificationCode: string;
      hostName: string;
      expiresAt: number;
    }
  | { type: "client-done" }
  | { type: "failed"; message: string }
  | { type: "reset" };

export const INITIAL_REMOTE_V3_PAIRING_UI: RemoteV3PairingUiState = { kind: "idle" };

export function reduceRemoteV3Pairing(
  state: RemoteV3PairingUiState,
  action: RemoteV3PairingUiAction,
): RemoteV3PairingUiState {
  switch (action.type) {
    case "reset":
      return INITIAL_REMOTE_V3_PAIRING_UI;
    case "failed":
      return { kind: "error", message: action.message };
    case "host-started":
      return {
        kind: "host",
        pairingId: action.pairingId,
        ticket: action.ticket,
        expiresAt: action.expiresAt,
        claimed: false,
        code: "",
      };
    case "host-claimed":
      return state.kind === "host" ? { ...state, claimed: true } : state;
    case "host-code":
      return state.kind === "host" && state.claimed ? { ...state, code: action.code } : state;
    case "host-done":
      return state.kind === "host" && state.claimed ? { kind: "host-done" } : state;
    case "client-claimed":
      return {
        kind: "client",
        pairingId: action.pairingId,
        verificationCode: action.verificationCode,
        hostName: action.hostName,
        expiresAt: action.expiresAt,
      };
    case "client-done":
      return state.kind === "client" ? { kind: "client-done" } : state;
  }
}
