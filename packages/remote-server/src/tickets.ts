import { TICKET_TTL_MS } from "@mode/remote-core";

import { signTicket, verifyTicket } from "./crypto.js";

export interface RelayTicketPayload {
  v: 1;
  aud: "relay";
  connectionId: string;
  deviceId: string;
  peerDeviceId: string;
  nodeId: string;
  epoch: number;
  exp: number;
}

export interface DirectTicketPayload {
  v: 1;
  aud: "direct";
  connectionId: string;
  hostDeviceId: string;
  clientDeviceId: string;
  exp: number;
}

export function createRelayTicket(
  secret: string,
  input: Omit<RelayTicketPayload, "v" | "aud" | "exp">,
  now = Date.now(),
): string {
  return signTicket(secret, {
    v: 1,
    aud: "relay",
    ...input,
    exp: now + TICKET_TTL_MS,
  } satisfies RelayTicketPayload);
}

export function createDirectTicket(
  secret: string,
  input: Omit<DirectTicketPayload, "v" | "aud" | "exp">,
  now = Date.now(),
): string {
  return signTicket(secret, {
    v: 1,
    aud: "direct",
    ...input,
    exp: now + TICKET_TTL_MS,
  } satisfies DirectTicketPayload);
}

export function buildDirectAssignment(
  secret: string,
  input: Omit<DirectTicketPayload, "v" | "aud" | "exp">,
  addresses: string[],
): { ticket: string; addresses: string[] } | undefined {
  if (addresses.length === 0) return undefined;
  return { ticket: createDirectTicket(secret, input), addresses };
}

export function verifyRelayTicket(
  secret: string,
  ticket: string,
  now = Date.now(),
): RelayTicketPayload | null {
  const payload = verifyTicket(secret, ticket);
  if (!payload || typeof payload !== "object") return null;
  const value = payload as Partial<RelayTicketPayload>;
  if (
    value.v !== 1 ||
    value.aud !== "relay" ||
    typeof value.connectionId !== "string" ||
    typeof value.deviceId !== "string" ||
    typeof value.peerDeviceId !== "string" ||
    typeof value.nodeId !== "string" ||
    typeof value.epoch !== "number" ||
    typeof value.exp !== "number" ||
    value.exp <= now
  ) {
    return null;
  }
  return value as RelayTicketPayload;
}

export function verifyDirectTicket(
  secret: string,
  ticket: string,
  now = Date.now(),
): DirectTicketPayload | null {
  const payload = verifyTicket(secret, ticket);
  if (!payload || typeof payload !== "object") return null;
  const value = payload as Partial<DirectTicketPayload>;
  if (
    value.v !== 1 ||
    value.aud !== "direct" ||
    typeof value.connectionId !== "string" ||
    typeof value.hostDeviceId !== "string" ||
    typeof value.clientDeviceId !== "string" ||
    typeof value.exp !== "number" ||
    value.exp <= now
  ) {
    return null;
  }
  return value as DirectTicketPayload;
}
