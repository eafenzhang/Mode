/**
 * 内嵌浏览器权限请求（主进程 → renderer 的弹层）与站点权限快照。
 *
 * 站点权限的权威状态在 main 的权限 store；renderer 只持有读取投影与用户操作进度，
 * 因此这里只定义跨进程的事件、决议与快照形状，不承载存储语义。
 */

/** 屏幕/窗口共享的可选源。 */
export interface EmbeddedBrowserScreenSource {
  sourceId: string;
  name: string;
  kind: "screen" | "window";
  thumbnailDataUrl?: string;
}

/** 可选的输入设备（hid / usb / serial）。 */
export interface EmbeddedBrowserInputDevice {
  deviceId: string;
  name?: string;
  vendorId?: number;
  productId?: number;
  serialNumber?: string;
}

interface EmbeddedBrowserPermissionPromptBase {
  requestId: string;
  origin: string;
  guestWebContentsId: number;
}

/**
 * 需要用户裁决的浏览器权限请求。
 *
 * `permission` 是 Chromium 的权限名（camera / microphone / clipboard-read 等）；
 * `media` 会用 `mediaTypes` 拆成摄像头与麦克风两行展示。
 */
export type EmbeddedBrowserPermissionPromptEvent =
  | (EmbeddedBrowserPermissionPromptBase & {
      kind: "permission";
      permission: string;
      mediaTypes?: readonly string[];
    })
  | (EmbeddedBrowserPermissionPromptBase & {
      kind: "screen-share";
      screens: readonly EmbeddedBrowserScreenSource[];
    })
  | (EmbeddedBrowserPermissionPromptBase & {
      kind: "device";
      /** 请求的设备类权限名（hid / usb / serial）。 */
      permission: string;
      devices: readonly EmbeddedBrowserInputDevice[];
    });

/**
 * 用户对某次请求的裁决。
 *
 * 不提供「永久阻止」：误点后站点异常难以自愈，持久 deny 只在站点权限设置里管理。
 */
export type EmbeddedBrowserPermissionResolution =
  | { action: "dismiss" | "allow-always" | "allow-session" | "cancel" }
  | { action: "share-screen"; sourceId: string }
  | { action: "select-device"; deviceId: string };

/** 单个站点上一条权限的持久状态；`ask` 表示回到每次询问。 */
export type EmbeddedBrowserSitePermissionState = "allow" | "deny" | "ask";

/** origin → permission → 状态。 */
export type EmbeddedBrowserSitePermissionsSnapshot = Record<
  string,
  Record<string, "allow" | "deny">
>;
