/** 远程控制 v3 线协议限额。任何超限都必须 fail closed。 */
export const PROTOCOL_VERSION = 1 as const;

export const MAX_CONTROL_FRAME_BYTES = 64 * 1024;
export const MAX_RELAY_FRAME_BYTES = 1024 * 1024;
export const MAX_NOISE_MESSAGE_BYTES = 65_535;
export const SECURE_FRAGMENT_PAYLOAD_BYTES = 48 * 1024;
export const MAX_SECURE_MESSAGE_BYTES = 4 * 1024 * 1024;
export const MAX_CONCURRENT_REASSEMBLIES = 8;

export const HELLO_TIMEOUT_MS = 5_000;
export const HEARTBEAT_INTERVAL_MS = 25_000;
export const RELAY_ATTACH_TIMEOUT_MS = 10_000;
export const DEFAULT_RECONNECT_CAP_MS = 30_000;

export const DEVICE_NAME_MAX_BYTES = 128;
export const PLATFORM_MAX_BYTES = 64;
export const VERSION_MAX_BYTES = 64;
export const IDENTITY_KEY_BYTES = 32;
export const TICKET_TTL_MS = 300_000;

export const CONTROL_MAX_BUFFERED_BYTES = 1024 * 1024;
export const RELAY_MAX_BUFFERED_BYTES = 4 * 1024 * 1024;
