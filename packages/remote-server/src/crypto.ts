import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";

import { decodeBase64Url, encodeBase64Url } from "@mode/remote-core";

export function sha256Hex(value: string | Uint8Array): string {
  return createHash("sha256").update(value).digest("hex");
}

export function randomToken(bytes = 32): string {
  return encodeBase64Url(randomBytes(bytes));
}

export function safeEqualHex(left: string, right: string): boolean {
  if (left.length !== right.length) return false;
  try {
    return timingSafeEqual(Buffer.from(left, "hex"), Buffer.from(right, "hex"));
  } catch {
    return false;
  }
}

export function safeEqualText(left: string, right: string): boolean {
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  return a.length === b.length && timingSafeEqual(a, b);
}

export function signTicket(secret: string, payload: unknown): string {
  const body = encodeBase64Url(Buffer.from(JSON.stringify(payload), "utf8"));
  const mac = createHmac("sha256", secret).update(body).digest();
  return `${body}.${encodeBase64Url(mac)}`;
}

export function verifyTicket(secret: string, ticket: string): unknown {
  const [body, mac, extra] = ticket.split(".");
  if (!body || !mac || extra !== undefined) return null;
  const expected = createHmac("sha256", secret).update(body).digest();
  let provided: Uint8Array;
  try {
    provided = decodeBase64Url(mac, "ticket.mac");
  } catch {
    return null;
  }
  if (provided.length !== expected.length || !timingSafeEqual(Buffer.from(provided), expected)) {
    return null;
  }
  try {
    return JSON.parse(Buffer.from(decodeBase64Url(body, "ticket.body")).toString("utf8"));
  } catch {
    return null;
  }
}
