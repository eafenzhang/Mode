// 本文件由 scripts/generate-tauri-protocol.mjs 生成，请勿手改。
//
// 真源：crates/protocol（Rust，schemars）。改动协议请改 Rust 侧类型后重新生成：
//   node scripts/generate-tauri-protocol.mjs
// CI 用 `--check` 断言生成物无漂移。

export const DEEP_LINK_EVENT = "mode:deep-link";

export interface ApplicationIconInfo {
  "iconDataUrl": string;
}

export type ApplicationIconLocator = {
  "kind": "darwin-bundle-id";
  "value": string;
} | {
  "kind": "windows-executable-path";
  "value": string;
} | {
  "kind": "windows-aumid";
  "value": string;
};

export interface ApplicationIconRequest {
  "locators": ApplicationIconLocator[];
}

export interface DeepLinkEvent {
  "code"?: string;
  "kind": DeepLinkEventKind;
  "path"?: string;
  "url": string;
}

export type DeepLinkEventKind = "oauth-callback" | "payment-callback" | "workspace-open" | "share-import" | "unrecognized";

export interface OpenPathResult {
  "error"?: string;
  "success": boolean;
}

export interface SaveFileRequest {
  "data"?: number[];
  "sourceUrl"?: string;
  "suggestedName": string;
}

export interface SaveFileResult {
  "canceled"?: boolean;
  "error"?: string;
  "path"?: string;
  "success": boolean;
}

export interface TaskNotificationPayload {
  "body": string;
  "requestId"?: string;
  "status": TaskNotificationStatus;
  "taskId": string;
  "title": string;
}

export type TaskNotificationStatus = "completed" | "failed" | "permission_request" | "elicitation_request" | "feedback_update";
