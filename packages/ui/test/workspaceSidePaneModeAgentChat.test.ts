import assert from "node:assert/strict";
import test from "node:test";
import {
  closeDraftModeAgentChatTabs,
  openModeAgentChatSidePane,
  updateModeAgentChatSidePaneSessionId,
} from "../src/lib/workspaceSidePane.js";

const base = {
  workspaceKey: "D:\\ZCodium",
  workspacePath: "D:\\ZCodium",
};

const deviceA = {
  serverName: "mode-agent-a",
  agentId: "a",
  name: "PC-A",
  host: "192.168.1.10",
  port: 47821,
};

const deviceB = {
  serverName: "mode-agent-b",
  agentId: "b",
  name: "PC-B",
  host: "192.168.1.11",
  port: 47821,
};

test("设备对话 tab 初始为草稿，同一设备重复打开保留 session", () => {
  const first = openModeAgentChatSidePane(null, { ...base, device: deviceA });
  assert.equal(first.tabs.length, 1);
  const tab = first.tabs[0];
  assert.equal(tab?.type, "mode-agent-chat");
  assert.equal(tab?.type === "mode-agent-chat" ? tab.sessionId : "x", null);

  const bound = updateModeAgentChatSidePaneSessionId(first, first.activeTabId, "sess-1");
  assert.ok(bound);
  const reopened = openModeAgentChatSidePane(bound, { ...base, device: deviceA });
  const reopenedTab = reopened.tabs[0];
  assert.equal(reopenedTab?.type === "mode-agent-chat" ? reopenedTab.sessionId : "x", "sess-1");
  assert.equal(reopenedTab?.id, tab?.id);
});

test("切换到另一台设备复用同一个 tab 并重置为草稿", () => {
  const first = openModeAgentChatSidePane(null, { ...base, device: deviceA });
  const bound = updateModeAgentChatSidePaneSessionId(first, first.activeTabId, "sess-1");
  assert.ok(bound);
  const switched = openModeAgentChatSidePane(bound, { ...base, device: deviceB });
  assert.equal(switched.tabs.length, 1);
  const tab = switched.tabs[0];
  assert.equal(tab?.type === "mode-agent-chat" ? tab.device.serverName : "x", deviceB.serverName);
  assert.equal(tab?.type === "mode-agent-chat" ? tab.sessionId : "x", null);
});

test("离开远程控制页只关闭未首发的草稿 tab", () => {
  const draft = openModeAgentChatSidePane(null, { ...base, device: deviceA });
  assert.equal(closeDraftModeAgentChatTabs(draft)?.tabs.length, 0);

  const bound = updateModeAgentChatSidePaneSessionId(draft, draft.activeTabId, "sess-1");
  assert.ok(bound);
  assert.equal(closeDraftModeAgentChatTabs(bound), bound);
});
