import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_REMOTE_DESKTOP_TICKET_TTL_MS,
  createRemoteDesktopTicketStore,
} from "../src/remoteDesktopTicket.js";

// 远程桌面会话票据（spec: docs/specs/lan-remote-desktop.md 状态所有者）：
// 30s TTL、单次消费、绑定签发时的 clientId。票据只在被控 host 进程内存中存在。

test("票据：同一 clientId 首次消费成功，重放立即失效", () => {
  const store = createRemoteDesktopTicketStore();
  const { ticket, expiresAt } = store.issue("client-a");
  assert.ok(ticket.length > 0, "票据必须是非空串");
  assert.equal(expiresAt > Date.now(), true, "TTL 必须是未来的时间点");
  assert.equal(store.consume(ticket, "client-a"), true, "首次消费必须成功");
  assert.equal(store.consume(ticket, "client-a"), false, "重放必须失败");
});

test("票据：绑定 clientId，其它客户端出示即拒绝并作废该票据", () => {
  const store = createRemoteDesktopTicketStore();
  const { ticket } = store.issue("client-a");
  assert.equal(store.consume(ticket, "client-b"), false, "错误 clientId 必须拒绝");
  // 错客户端尝试也消费票据：合法持有者不能再用它（同一重放纪律）。
  assert.equal(store.consume(ticket, "client-a"), false, "被错客户端试过之后不得复活");
});

test("票据：缺 clientId（令牌解析不到设备）一律拒绝", () => {
  const store = createRemoteDesktopTicketStore();
  const { ticket } = store.issue("client-a");
  assert.equal(store.consume(ticket, undefined), false);
});

test("票据：未出示、空串与未知票据一律拒绝", () => {
  const store = createRemoteDesktopTicketStore();
  store.issue("client-a");
  assert.equal(store.consume(undefined, "client-a"), false);
  assert.equal(store.consume("", "client-a"), false);
  assert.equal(store.consume("not-issued", "client-a"), false);
});

test("票据：TTL 内有效、到期即失效（注入时钟）", () => {
  let now = 1_000;
  const store = createRemoteDesktopTicketStore({ now: () => now });
  const { ticket } = store.issue("client-a");
  now += DEFAULT_REMOTE_DESKTOP_TICKET_TTL_MS - 1;
  assert.equal(store.consume(ticket, "client-a"), true, "到期前 1ms 仍应有效");

  now = 50_000;
  const second = store.issue("client-a").ticket;
  now += DEFAULT_REMOTE_DESKTOP_TICKET_TTL_MS;
  assert.equal(store.consume(second, "client-a"), false, "到期时刻起失效");
});

test("票据：极短 TTL 构造下过期（不依赖真实时钟推进）", async () => {
  const store = createRemoteDesktopTicketStore({ ttlMs: 1 });
  const { ticket } = store.issue("client-a");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(store.consume(ticket, "client-a"), false);
});

test("票据：issue 返回的票据互不相同（不可预测的随机串，注入构造器可控）", () => {
  let counter = 0;
  const store = createRemoteDesktopTicketStore({ createTicket: () => `ticket-${counter++}` });
  assert.equal(store.issue("client-a").ticket, "ticket-0");
  assert.equal(store.issue("client-b").ticket, "ticket-1");
});
