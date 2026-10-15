/**
 * 每设备连接选项的前端纯逻辑（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 9）。
 *
 * 形状真源是 `@mode/shared/remote-desktop-rdp` 的 `RdpConnectionOptions`：所有字段可选，
 * 缺省语义由这里补齐（UI 与缺省值只有这一份「读缺省」的实现，避免卡片/弹窗各写一套）。
 * 真正写入 `.rdp` 文件的 `buildRdpFile` 在主控 main（本包不生成 `.rdp`）。
 */
import {
  DEFAULT_RDP_PORT,
  type RdpAudioMode,
  type RdpConnectionOptions,
} from "@mode/shared/remote-desktop-rdp";

/** 选项补齐后的形态：UI 控件需要确定值，不能到处写 `?? default`。 */
export interface ResolvedRdpOptions {
  fullscreen: boolean;
  multimon: boolean;
  clipboard: boolean;
  audio: RdpAudioMode;
  username: string;
  port: number;
}

/** 缺省值（spec 产品规则 9）：窗口模式、多显示器关、剪贴板开、音频带到本机、端口 3389。 */
export const RDP_OPTION_DEFAULTS: ResolvedRdpOptions = {
  fullscreen: false,
  multimon: false,
  clipboard: true,
  audio: "local",
  username: "",
  port: DEFAULT_RDP_PORT,
};

/** 音频重定向目标（UI 呈现顺序：带到本机 / 留在被控 / 禁用）。 */
export const RDP_AUDIO_MODES: readonly RdpAudioMode[] = ["local", "remote", "disabled"];

/**
 * 把持久化的（可能只存了部分字段的）选项补齐为 UI 可用形态。
 * 未知音频值按缺省 local 兜底，端口越界回退 3389——避免把脏数据带进启动器。
 */
export function resolveRdpOptions(
  options: RdpConnectionOptions | undefined | null,
): ResolvedRdpOptions {
  const audio = options?.audio;
  const port = options?.port;
  return {
    fullscreen: options?.fullscreen ?? RDP_OPTION_DEFAULTS.fullscreen,
    multimon: options?.multimon ?? RDP_OPTION_DEFAULTS.multimon,
    clipboard: options?.clipboard ?? RDP_OPTION_DEFAULTS.clipboard,
    audio: audio && RDP_AUDIO_MODES.includes(audio) ? audio : RDP_OPTION_DEFAULTS.audio,
    username: options?.username ?? RDP_OPTION_DEFAULTS.username,
    port:
      typeof port === "number" && Number.isInteger(port) && port >= 1 && port <= 65535
        ? port
        : RDP_OPTION_DEFAULTS.port,
  };
}

/** 提交前收敛：用户名去首尾空白，空串不写（等价于不预填）。 */
export function toRdpConnectionOptions(resolved: ResolvedRdpOptions): RdpConnectionOptions {
  const username = resolved.username.trim();
  return {
    fullscreen: resolved.fullscreen,
    multimon: resolved.multimon,
    clipboard: resolved.clipboard,
    audio: resolved.audio,
    ...(username ? { username } : {}),
    port: resolved.port,
  };
}
