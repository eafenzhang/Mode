import type {
  BotWecomRegistrationBeginResult,
  BotWecomRegistrationPollParams,
  BotWecomRegistrationPollResult,
} from "../bots.js";

/**
 * 企业微信智能机器人一键创建（扫码授权）。
 *
 * 使用企业微信公开的智能机器人 QR 接口（与官方 @wecom/wecom-openclaw-cli 相同的流程）：
 * - generate：返回 scode（轮询凭据）与 auth_url（渲染为二维码）
 * - query_result：轮询扫码状态；success 时返回 bot_info.{botid, secret}
 *
 * 这两次请求都会访问外部 HTTPS 主机（work.weixin.qq.com），失败按 HTTP/业务码分层报错，
 * 方便 UI 区分“网络问题”与“接口拒绝”。
 */

type WeComRegistrationBeginResult = BotWecomRegistrationBeginResult;
type WeComRegistrationPollParams = BotWecomRegistrationPollParams;
type WeComRegistrationPollResult = BotWecomRegistrationPollResult;

const WECOM_QR_BASE_URL = "https://work.weixin.qq.com/ai/qc";
/** 二维码有效期：企业微信未返回过期时间，按 10 分钟兜底（与扫码授权体验匹配）。 */
const WECOM_QR_TTL_MS = 10 * 60 * 1000;
const WECOM_QR_POLL_INTERVAL_SECONDS = 2;
const WECOM_QR_REQUEST_TIMEOUT_MS = 15_000;
/** source 用于企业微信后台统计来源，固定为本产品标识。 */
const WECOM_QR_SOURCE = "mode";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function readString(record: Record<string, unknown> | null | undefined, key: string): string {
  const value = record?.[key];
  return typeof value === "string" ? value.trim() : "";
}

function resolveWeComPlatformCode(): number {
  if (process.platform === "darwin") return 1;
  if (process.platform === "win32") return 2;
  return 3;
}

async function fetchWeComQrJson(path: string): Promise<Record<string, unknown>> {
  const response = await fetch(`${WECOM_QR_BASE_URL}${path}`, {
    method: "GET",
    signal: AbortSignal.timeout(WECOM_QR_REQUEST_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`WeCom registration request failed: HTTP ${response.status}`);
  }
  const payload: unknown = await response.json();
  if (!isRecord(payload)) {
    throw new Error("WeCom registration returned a non-object payload.");
  }
  const errcode = payload.errcode;
  if (typeof errcode === "number" && errcode !== 0) {
    throw new Error(
      `WeCom registration API error ${errcode}: ${readString(payload, "errmsg") || "unknown error"}`,
    );
  }
  return isRecord(payload.data) ? payload.data : payload;
}

/**
 * scode 参与 URL 拼接，只保留 URL-safe 字母表（纵深防御，防止注入）。
 *
 * 修复原因（2026-10-20 真机验收）：企微 generate 返回的 scode 是 URL-safe 字符串，
 * 实测含 `-` 与 `_`（如 `dF3l53w8JGOL_tOf`）。此前只保留 `[a-zA-Z0-9]` 会把这两个
 * 有效字符删掉，query_result 实际查的是另一个 scode，永远返回 pending，
 * 扫码后 UI 一直停在"等待扫码"。scode 只用于 query 参数，允许 `-`/`_` 不构成注入面。
 */
export function sanitizeWeComScode(scode: string): string {
  return scode.replace(/[^a-zA-Z0-9_-]/gu, "");
}

export async function beginWeComRegistration(): Promise<WeComRegistrationBeginResult> {
  const data = await fetchWeComQrJson(
    `/generate?source=${encodeURIComponent(WECOM_QR_SOURCE)}&plat=${resolveWeComPlatformCode()}`,
  );
  const scode = readString(data, "scode");
  const authUrl = readString(data, "auth_url");
  if (!scode || !authUrl) {
    throw new Error("WeCom registration did not return scode / auth_url.");
  }
  return {
    scode,
    authUrl,
    interval: WECOM_QR_POLL_INTERVAL_SECONDS,
    expiresAt: Date.now() + WECOM_QR_TTL_MS,
  };
}

export async function pollWeComRegistration(
  params: WeComRegistrationPollParams,
): Promise<WeComRegistrationPollResult> {
  const safeScode = sanitizeWeComScode(params.scode);
  if (!safeScode) {
    return { status: "error", message: "Invalid scode." };
  }
  const data = await fetchWeComQrJson(`/query_result?scode=${encodeURIComponent(safeScode)}`);
  const status = readString(data, "status") || "waiting";
  switch (status) {
    case "success": {
      const botInfo = isRecord(data.bot_info) ? data.bot_info : {};
      const botId = readString(botInfo, "botid");
      const secret = readString(botInfo, "secret");
      if (!botId || !secret) {
        return {
          status: "error",
          message: "WeCom scan succeeded but bot_info is incomplete.",
        };
      }
      return { status: "success", botId, secret };
    }
    case "expired":
      return { status: "expired" };
    case "cancelled":
      return { status: "cancelled" };
    case "denied":
      return { status: "denied" };
    case "scaned":
    case "scanned":
      return { status: "scanned", interval: WECOM_QR_POLL_INTERVAL_SECONDS };
    case "waiting":
    default:
      return { status: "pending", interval: WECOM_QR_POLL_INTERVAL_SECONDS };
  }
}
