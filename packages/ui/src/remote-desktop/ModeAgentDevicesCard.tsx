/* eslint-disable max-lines -- pairing, desktop-code, device list and chat entry share one MCP-store owner; splitting would duplicate state. */
import { useCallback, useState } from "react";
import {
  LoaderIcon,
  MessageSquarePlus,
  Plus,
  TerminalSquare,
  Trash2,
  Wifi,
  WifiOff,
} from "lucide-react";
import type { McpServerStatus } from "@mode/shared";
import type { CreateTaskRequest } from "@/app-shell/types.js";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { toast } from "@/ui-foundation/adapters/toast.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useServices } from "@/hooks/useServices.js";
import { logger } from "@/logger.js";
import {
  buildDesktopEnrollmentUri,
  buildModeAgentLaunchPrompt,
  buildModeAgentMcpConfig,
  modeAgentServerName,
  parseModeAgentPairingText,
  pairModeAgent,
  readModeAgentServers,
  type ModeAgentServerRecord,
} from "@/remote-desktop/modeAgentPairing.js";
import { useMcpStore } from "@/store/mcpStore.js";

const DEFAULT_PORT = "47821";

function formatCode(value: string): string {
  const normalized = value.toUpperCase().replace(/[^0-9A-Z]/gu, "").slice(0, 8);
  return normalized.length > 4 ? `${normalized.slice(0, 4)}-${normalized.slice(4)}` : normalized;
}

function statusLabelId(status: McpServerStatus | undefined): string {
  switch (status) {
    case "connected":
      return "remoteDesktop.modeAgents.statusConnected";
    case "connecting":
      return "remoteDesktop.modeAgents.statusConnecting";
    case "error":
    case "disconnected":
      return "remoteDesktop.modeAgents.statusOffline";
    default:
      return "remoteDesktop.modeAgents.statusUnknown";
  }
}

function statusClass(status: McpServerStatus | undefined): string {
  switch (status) {
    case "connected":
      return "text-success";
    case "connecting":
      return "text-warning";
    case "error":
      return "text-destructive";
    default:
      return "text-foreground-subtle";
  }
}

/**
 * 远程控制页 · Mode Agent 被控端（docs/specs/legacy-windows-remote-agent.md）：
 * - 主路径：桌面生成配对码 + 配对串，被控端安装弹窗输入后反向登记；
 * - 备用：被控端显示配对码，桌面输入（旧方向）；
 * - 列表直接读 MCP store（单一事实源），发起对话只预填目标与工具约束，不自动发送。
 */
export function ModeAgentDevicesCard({
  onCreateTask,
}: {
  onCreateTask?: (request?: CreateTaskRequest) => void;
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const services = useServices();
  const servers = useMcpStore((state) => state.servers);
  const records = readModeAgentServers(servers);
  const [expanded, setExpanded] = useState(false);
  const [pairingText, setPairingText] = useState("");
  const [host, setHost] = useState("");
  const [port, setPort] = useState(DEFAULT_PORT);
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [error, setError] = useState("");
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [desktopCode, setDesktopCode] = useState("");
  const [desktopUri, setDesktopUri] = useState("");
  const [desktopBusy, setDesktopBusy] = useState(false);
  const [desktopError, setDesktopError] = useState("");

  const generateDesktopCode = useCallback(async () => {
    setDesktopBusy(true);
    setDesktopError("");
    try {
      let state = await platform.getLanAccessState();
      if (!state.enabled) {
        state = await platform.setLanAccessEnabled(true);
      }
      state = await platform.createLanAccessPairCode();
      const pair = state.pairCode;
      if (!pair) {
        throw new Error(intl.formatMessage({ id: "remoteDesktop.modeAgents.desktopCodeFailed" }));
      }
      const address = state.addresses[0] ?? "";
      setDesktopCode(pair.code);
      setDesktopUri(buildDesktopEnrollmentUri(address, state.port, pair.code));
      setExpanded(false);
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setDesktopError(message);
      toast(
        `${intl.formatMessage({ id: "remoteDesktop.modeAgents.desktopCodeFailed" })}：${message}`,
      );
    } finally {
      setDesktopBusy(false);
    }
  }, [intl, platform]);

  const copyDesktopUri = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(desktopUri);
      toast(intl.formatMessage({ id: "remoteDesktop.modeAgents.copied" }));
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      toast(`${intl.formatMessage({ id: "remoteDesktop.modeAgents.copyFailed" })}：${message}`);
    }
  }, [desktopUri, intl]);

  const refreshAgents = useCallback(async () => {
    try {
      const state = useMcpStore.getState();
      await state.loadMcpFromUserDirectory(services.mcpSyncService, state.currentWorkspaceIdentity);
      toast(intl.formatMessage({ id: "remoteDesktop.modeAgents.refreshed" }));
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      toast(`${intl.formatMessage({ id: "remoteDesktop.modeAgents.refreshFailed" })}：${message}`);
    }
  }, [intl, services.mcpSyncService]);

  const applyPairingText = useCallback((value: string) => {
    setPairingText(value);
    if (!value.trim()) return;
    const parsed = parseModeAgentPairingText(value);
    if (parsed.host) setHost(parsed.host);
    if (parsed.port) setPort(String(parsed.port));
    if (parsed.code) setCode(formatCode(parsed.code));
  }, []);

  const pair = useCallback(async () => {
    const numericPort = Number.parseInt(port, 10);
    setPairing(true);
    setError("");
    try {
      const result = await pairModeAgent({
        host,
        port: numericPort,
        code,
        label: "Mode Desktop",
      });
      const serverName = modeAgentServerName(result.agentId);
      const config = buildModeAgentMcpConfig(result, {
        host,
        port: numericPort,
        code,
      });
      const state = useMcpStore.getState();
      const existing = state.servers.find((server) => server.name === serverName);
      if (existing) {
        await state.updateScopedMcpServer("zcodeagentmcp", serverName, config);
      } else {
        await state.addScopedMcpServer("zcodeagentmcp", serverName, config);
      }
      toast(
        intl.formatMessage(
          { id: "remoteDesktop.modeAgents.pairSuccess" },
          { name: result.name || `${host}:${numericPort}` },
        ),
      );
      setExpanded(false);
      setPairingText("");
      setHost("");
      setPort(DEFAULT_PORT);
      setCode("");
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      toast(
        `${intl.formatMessage({ id: "remoteDesktop.modeAgents.pairFailed" })}：${message}`,
      );
    } finally {
      setPairing(false);
    }
  }, [code, host, intl, port]);

  const startChat = useCallback(
    async (record: ModeAgentServerRecord) => {
      setError("");
      try {
        const state = useMcpStore.getState();
        const server = state.servers.find((entry) => entry.name === record.serverName);
        if (server && !(state.enabledStates[server.id] ?? server.enabled)) {
          await state.toggleServer(server.id, true);
        }
        if (!onCreateTask) {
          throw new Error(intl.formatMessage({ id: "remoteDesktop.modeAgents.chatUnavailable" }));
        }
        onCreateTask({
          initialPrompt: buildModeAgentLaunchPrompt(record),
        });
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        setError(message);
        toast(message);
      }
    },
    [intl, onCreateTask],
  );

  const remove = useCallback(
    (record: ModeAgentServerRecord) => {
      if (removeTarget !== record.serverName) {
        setRemoveTarget(record.serverName);
        return;
      }
      try {
        useMcpStore.getState().deleteScopedMcpServer("zcodeagentmcp", record.serverName);
        setRemoveTarget(null);
        toast(
          intl.formatMessage(
            { id: "remoteDesktop.modeAgents.removeSuccess" },
            { name: record.name },
          ),
        );
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        logger.warn("[mode-agent] remove paired device failed", message);
        setError(message);
      }
    },
    [intl, removeTarget],
  );

  return (
    <section
      className="rounded-xl border border-border bg-card"
      data-testid="mode-agent-devices-card"
    >
      <header className="flex flex-wrap items-start justify-between gap-3 px-4 py-3">
        <div className="min-w-0">
          <h2 className="flex items-center gap-2 text-ui-lg font-medium text-foreground">
            <TerminalSquare className="size-4" />
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.title" })}
          </h2>
          <p className="mt-1 text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.description" })}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="mode-agent-generate-code"
            disabled={desktopBusy}
            onClick={() => void generateDesktopCode()}
          >
            {desktopBusy ? (
              <LoaderIcon className="size-3.5 animate-spin" />
            ) : (
              <Plus className="size-4" />
            )}
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.generateCode" })}
          </Button>
          <Button
            type="button"
            variant={expanded ? "ghost" : "outline"}
            size="sm"
            data-testid="mode-agent-add-device"
            onClick={() => {
              setExpanded((previous) => !previous);
              setError("");
            }}
          >
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.manualPair" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-testid="mode-agent-refresh"
            onClick={() => void refreshAgents()}
          >
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.refreshList" })}
          </Button>
        </div>
      </header>

      {desktopCode ? (
        <div className="space-y-3 border-t border-border px-4 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-ui-base text-foreground">
              {intl.formatMessage({ id: "remoteDesktop.modeAgents.desktopCodeTitle" })}：
            </span>
            <span className="font-mono text-ui-lg tracking-[0.3em] text-foreground">
              {desktopCode}
            </span>
            <span className="text-ui-xs text-foreground-subtlest">
              {intl.formatMessage({ id: "remoteDesktop.modeAgents.desktopCodeExpiry" })}
            </span>
          </div>
          <Input
            readOnly
            size="lg"
            value={desktopUri}
            data-testid="mode-agent-pairing-uri"
            onFocus={(event) => event.currentTarget.select()}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void copyDesktopUri()}
            >
              {intl.formatMessage({ id: "remoteDesktop.modeAgents.copyPairUri" })}
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => void refreshAgents()}>
              {intl.formatMessage({ id: "remoteDesktop.modeAgents.refreshList" })}
            </Button>
          </div>
          <p className="text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.desktopCodeHint" })}
          </p>
          {desktopError ? <p className="text-ui-base text-destructive">{desktopError}</p> : null}
        </div>
      ) : null}

      {expanded ? (
        <div className="space-y-3 border-t border-border px-4 py-3">
          <p className="text-ui-xs text-foreground-subtlest">
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.manualPairHint" })}
          </p>
          <Input
            size="lg"
            value={pairingText}
            data-testid="mode-agent-pairing-text"
            placeholder="mode-agent://pair?host=192.168.1.20&port=47821&code=ABCD-EFGH"
            onChange={(event) => applyPairingText(event.target.value)}
          />
          <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_8rem_10rem_auto]">
            <label className="space-y-1 text-ui-xs text-foreground-subtle">
              <span>{intl.formatMessage({ id: "remoteDesktop.modeAgents.address" })}</span>
              <Input
                size="lg"
                value={host}
                data-testid="mode-agent-host"
                placeholder="192.168.1.20"
                onChange={(event) => setHost(event.target.value)}
              />
            </label>
            <label className="space-y-1 text-ui-xs text-foreground-subtle">
              <span>{intl.formatMessage({ id: "remoteDesktop.modeAgents.port" })}</span>
              <Input
                size="lg"
                value={port}
                inputMode="numeric"
                data-testid="mode-agent-port"
                onChange={(event) => setPort(event.target.value.replace(/[^0-9]/gu, ""))}
              />
            </label>
            <label className="space-y-1 text-ui-xs text-foreground-subtle">
              <span>{intl.formatMessage({ id: "remoteDesktop.modeAgents.code" })}</span>
              <Input
                size="lg"
                value={code}
                data-testid="mode-agent-code"
                placeholder="ABCD-EFGH"
                className="tracking-[0.2em] uppercase"
                onChange={(event) => setCode(formatCode(event.target.value))}
              />
            </label>
            <div className="flex items-end">
              <Button
                type="button"
                variant="outline"
                size="lg"
                data-testid="mode-agent-pair"
                disabled={pairing || !host.trim() || code.replace(/-/gu, "").length !== 8}
                onClick={() => void pair()}
              >
                {pairing ? <LoaderIcon className="size-3.5 animate-spin" /> : null}
                {intl.formatMessage({
                  id: pairing
                    ? "remoteDesktop.modeAgents.pairing"
                    : "remoteDesktop.modeAgents.pair",
                })}
              </Button>
            </div>
          </div>
          {error ? <p className="text-ui-base text-destructive">{error}</p> : null}
        </div>
      ) : null}

      <div className="border-t border-border px-4 py-3">
        {records.length === 0 ? (
          <p className="rounded-lg border border-dashed border-border px-3 py-2 text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.empty" })}
          </p>
        ) : (
          <ul className="space-y-2" data-testid="mode-agent-device-list">
            {records.map((record) => {
              const status = record.server.status ?? "unknown";
              const toolCount = record.server.toolCount;
              return (
                <li
                  key={record.serverName}
                  className="flex flex-wrap items-start justify-between gap-3 rounded-lg border border-border bg-background px-3 py-2"
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-ui-base text-foreground">{record.name}</span>
                      <span
                        className={`inline-flex items-center gap-1 text-ui-xs ${statusClass(status)}`}
                      >
                        {status === "connected" ? (
                          <Wifi className="size-3" />
                        ) : (
                          <WifiOff className="size-3" />
                        )}
                        {intl.formatMessage({ id: statusLabelId(status) })}
                      </span>
                    </div>
                    <p className="mt-0.5 truncate font-mono text-ui-xs text-foreground-subtle">
                      {record.host}:{record.port}
                    </p>
                    <p className="mt-0.5 truncate text-ui-xs text-foreground-subtlest">
                      {intl.formatMessage(
                        { id: "remoteDesktop.modeAgents.mcpInstalled" },
                        { server: record.serverName },
                      )}
                      {typeof toolCount === "number"
                        ? ` · ${intl.formatMessage(
                            { id: "remoteDesktop.modeAgents.tools" },
                            { count: toolCount },
                          )}`
                        : ""}
                    </p>
                    {record.server.error ? (
                      <p className="mt-0.5 truncate text-ui-xs text-destructive">
                        {record.server.error}
                      </p>
                    ) : null}
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid="mode-agent-start-chat"
                      onClick={() => void startChat(record)}
                    >
                      <MessageSquarePlus className="size-4" />
                      {intl.formatMessage({ id: "remoteDesktop.modeAgents.startChat" })}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      data-testid="mode-agent-remove"
                      onClick={() => remove(record)}
                    >
                      <Trash2 className="size-4" />
                      {removeTarget === record.serverName
                        ? intl.formatMessage({ id: "remoteDesktop.modeAgents.removeConfirm" })
                        : intl.formatMessage({ id: "remoteDesktop.modeAgents.remove" })}
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}
