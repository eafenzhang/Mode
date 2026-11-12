import { useCallback, useEffect, useMemo, useState } from "react";
import { openWebWorkspace } from "@mode/client";
import {
  WebAccountClient,
  WebControlLink,
  importWebIdentity,
  type WebAccountSummary,
  type WebDeviceProfile,
  type WebDeviceSummary,
  type WebGithubChallenge,
} from "@mode/remote-core/web";

import { SavedV3Accounts } from "./SavedV3Accounts.js";
import { WebV3RemoteDesktop } from "./WebV3RemoteDesktop.js";
import { createWebCredentialStore } from "./credentialStore.js";
import { deleteWebProfile } from "@mode/remote-core/web";

import { listStoredV3Accounts, V3_SERVER_URL_KEY } from "./storedProfile.js";

const credentialStore = createWebCredentialStore();

export function WebV3Page() {
  const [serverUrl, setServerUrl] = useState(() => localStorage.getItem(V3_SERVER_URL_KEY) ?? "");
  const [profile, setProfile] = useState<WebDeviceProfile | null>(null);
  const [hosts, setHosts] = useState<WebDeviceSummary[]>([]);
  const [challenge, setChallenge] = useState<WebGithubChallenge | null>(null);
  const [busy, setBusy] = useState(false);
  const [accounts, setAccounts] = useState<WebAccountSummary[]>([]);
  const [error, setError] = useState("");
  const [testResult, setTestResult] = useState("");
  const [rdTarget, setRdTarget] = useState<{
    link: WebControlLink;
    hostDeviceId: string;
    name: string;
  } | null>(null);

  const refreshAccounts = useCallback(async () => {
    setAccounts(await listStoredV3Accounts());
  }, []);

  useEffect(() => {
    void refreshAccounts();
  }, [refreshAccounts]);

  const client = useMemo(() => {
    const url = serverUrl.trim();
    return url
      ? new WebAccountClient({ store: credentialStore, serverUrl: url, deviceName: "Mode Web" })
      : null;
  }, [serverUrl]);

  const refreshHosts = useCallback(async () => {
    if (!client) return;
    try {
      const next = await client.listHosts();
      setHosts(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      setHosts([]);
    }
  }, [client]);

  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    void (async () => {
      try {
        const current = await client.getProfile();
        if (cancelled) return;
        // ensureProfile 会为首次访问生成设备身份；没有 accessToken 时仍是未登录，
        // 不能展示「已登录」区块，否则登录按钮会被隐藏、用户无从继续。
        if (!current.accessToken) return;
        setProfile(current);
        const next = await client.listHosts();
        if (!cancelled) setHosts(next);
      } catch {
        // 首次使用或服务地址不可达：保留登录表单。
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client]);

  useEffect(() => {
    if (!challenge || !client) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const poll = async (): Promise<void> => {
      try {
        const result = await client.pollGithubDeviceLogin(challenge.deviceCode);
        if (cancelled) return;
        if (result.status === "ok") {
          setProfile(result.profile);
          setChallenge(null);
          await refreshHosts();
          return;
        }
        if (result.status === "slow_down") {
          timer = setTimeout(poll, (result.interval ?? challenge.interval + 5) * 1000);
          return;
        }
        if (result.status === "expired" || result.status === "denied") {
          setError(result.status === "expired" ? "授权码已过期" : "授权被拒绝");
          setChallenge(null);
          return;
        }
        timer = setTimeout(poll, challenge.interval * 1000);
      } catch (caught) {
        if (cancelled) return;
        setError(caught instanceof Error ? caught.message : String(caught));
        setChallenge(null);
      }
    };
    timer = setTimeout(poll, challenge.interval * 1000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [challenge, client, refreshHosts]);

  const beginLogin = async (): Promise<void> => {
    if (!client) return;
    setBusy(true);
    setError("");
    setTestResult("");
    try {
      localStorage.setItem(V3_SERVER_URL_KEY, serverUrl.trim());
      setChallenge(await client.beginGithubDeviceLogin());
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const logout = async (): Promise<void> => {
    if (!client) return;
    await client.signOut();
    setProfile(null);
    setHosts([]);
    setError("");
    setTestResult("");
    await refreshAccounts();
  };

  const removeAccount = async (url: string): Promise<void> => {
    await deleteWebProfile(credentialStore, url);
    if (serverUrl.trim() === url) setServerUrl("");
    await refreshAccounts();
  };

  const testWorkspace = async (host: WebDeviceSummary): Promise<void> => {
    if (!profile?.accessToken) return;
    setBusy(true);
    setError("");
    setTestResult("正在建立工作区隧道…");
    let transport = "未知";
    const link = new WebControlLink({
      serverUrl: profile.serverUrl,
      deviceId: profile.deviceId,
      accessToken: profile.accessToken,
      identity: importWebIdentity(profile.privateKey),
      onTransport: (selected) => {
        transport = selected;
      },
    });
    try {
      await link.start();
      await link.connect(host.deviceId);
      const workspace = await openWebWorkspace({ link, hostDeviceId: host.deviceId });
      try {
        const info = await workspace.services.systemService.info();
        setTestResult(`传输：${transport}\n${JSON.stringify(info, null, 2)}`);
      } finally {
        workspace.dispose();
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      setTestResult("");
    } finally {
      link.close();
      setBusy(false);
    }
  };

  /** 进入远程桌面：先建 Control/Noise 连接，再交给会话面发起（同意窗在 Host 本机）。 */
  const enterDesktop = async (host: WebDeviceSummary): Promise<void> => {
    if (!profile?.accessToken) return;
    setBusy(true);
    setError("");
    setTestResult("");
    const link = new WebControlLink({
      serverUrl: profile.serverUrl,
      deviceId: profile.deviceId,
      accessToken: profile.accessToken,
      identity: importWebIdentity(profile.privateKey),
    });
    try {
      await link.start();
      await link.connect(host.deviceId);
      setRdTarget({ link, hostDeviceId: host.deviceId, name: host.name });
    } catch (caught) {
      link.close();
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const testConnection = async (host: WebDeviceSummary): Promise<void> => {
    if (!profile?.accessToken) return;
    setBusy(true);
    setError("");
    setTestResult("正在建立 Noise 通道…");
    let transport = "未知";
    const link = new WebControlLink({
      serverUrl: profile.serverUrl,
      deviceId: profile.deviceId,
      accessToken: profile.accessToken,
      identity: importWebIdentity(profile.privateKey),
      onTransport: (selected) => {
        transport = selected;
      },
    });
    try {
      await link.start();
      const connection = await link.connect(host.deviceId);
      const id = crypto.randomUUID();
      const response = await new Promise<unknown>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("等待 remote.host.info 超时")), 8_000);
        const dispose = connection.onFrame((frame) => {
          if (frame.type !== "rpc.response" || frame.id !== id) return;
          clearTimeout(timer);
          dispose();
          resolve(frame.result);
        });
        connection.send({ v: 1, type: "rpc.request", id, method: "remote.host.info", params: {} });
      });
      setTestResult(`传输：${transport}\n${JSON.stringify(response, null, 2)}`);
      connection.close();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      setTestResult("");
    } finally {
      link.close();
      setBusy(false);
    }
  };

  if (rdTarget) {
    return (
      <WebV3RemoteDesktop
        link={rdTarget.link}
        hostDeviceId={rdTarget.hostDeviceId}
        deviceName={rdTarget.name}
        onExit={() => {
          rdTarget.link.close();
          setRdTarget(null);
        }}
      />
    );
  }

  return (
    <div className="min-h-screen bg-background px-4 py-6 text-foreground sm:py-10">
      <div className="mx-auto max-w-2xl space-y-5">
        <header className="space-y-1">
          <h1 className="text-2xl font-semibold">远程控制 v3 · Web</h1>
          <p className="text-ui-base text-foreground-subtle">
            浏览器设备身份 + Noise 端到端加密；优先 WebRTC P2P，失败自动回落 Relay。
          </p>
        </header>

        <section className="space-y-3 rounded-xl border border-border bg-card p-4">
          <label className="block text-ui-xs text-foreground-subtle" htmlFor="v3-server-url">
            账号服务地址
          </label>
          <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap">
            <input
              id="v3-server-url"
              className="min-w-0 flex-1 rounded-lg border border-input-border bg-input px-3 py-2 text-ui-base outline-none focus:border-input-border-focused"
              value={serverUrl}
              placeholder="https://remote.example.com"
              autoComplete="off"
              spellCheck={false}
              disabled={busy || Boolean(profile)}
              onChange={(event) => setServerUrl(event.target.value)}
            />
            {profile ? (
              <button
                type="button"
                className="w-full rounded-lg border border-border px-3 py-2 text-ui-base hover:bg-surface-hover sm:w-auto"
                onClick={() => void logout()}
              >
                退出
              </button>
            ) : (
              <button
                type="button"
                className="w-full rounded-lg bg-primary px-3 py-2 text-ui-base text-primary-foreground disabled:opacity-50 sm:w-auto"
                disabled={busy || !client || !serverUrl.trim()}
                onClick={() => void beginLogin()}
              >
                使用 GitHub 登录
              </button>
            )}
          </div>

          {!profile ? (
            <SavedV3Accounts
              accounts={accounts}
              disabled={busy}
              onUse={(url) => {
                localStorage.setItem(V3_SERVER_URL_KEY, url);
                setServerUrl(url);
              }}
              onDelete={(url) => void removeAccount(url)}
            />
          ) : null}

          {challenge ? (
            <div className="space-y-2 rounded-lg border border-border bg-background p-3">
              <p className="text-ui-base text-foreground">在 GitHub 输入验证码：</p>
              <p className="text-2xl font-semibold tracking-[0.25em]">{challenge.userCode}</p>
              <a
                className="text-ui-base text-sky-600 hover:underline dark:text-sky-400"
                href={challenge.verificationUri}
                target="_blank"
                rel="noreferrer"
              >
                打开 {challenge.verificationUri}
              </a>
              <p className="text-ui-xs text-foreground-subtle">授权后本页会自动完成设备注册。</p>
            </div>
          ) : null}

          {error ? <p className="text-ui-base text-destructive">{error}</p> : null}
        </section>

        {profile ? (
          <section className="space-y-3 rounded-xl border border-border bg-card p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <p className="text-ui-base text-foreground">本机 Web 设备</p>
                <p className="text-ui-xs text-foreground-subtle">{profile.deviceId}</p>
              </div>
              <button
                type="button"
                className="rounded-lg border border-border px-3 py-1.5 text-ui-sm hover:bg-surface-hover"
                onClick={() => void refreshHosts()}
              >
                刷新设备
              </button>
            </div>
            {hosts.length === 0 ? (
              <p className="text-ui-base text-foreground-subtle">同账号下没有其他在线 Host。</p>
            ) : (
              <ul className="space-y-2">
                {hosts.map((host) => (
                  <li
                    key={host.deviceId}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-ui-base text-foreground">{host.name}</p>
                      <p className="text-ui-xs text-foreground-subtle">
                        {host.online ? "在线" : "离线"} · {host.role} · {host.platform}
                        {host.addresses[0] ? ` · ${host.addresses[0]}` : ""}
                      </p>
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      <button
                        type="button"
                        className="rounded-lg bg-primary px-3 py-1.5 text-ui-sm text-primary-foreground hover:opacity-90 disabled:opacity-50"
                        disabled={busy || !host.online}
                        onClick={() => void enterDesktop(host)}
                      >
                        进入桌面
                      </button>
                      <button
                        type="button"
                        className="rounded-lg border border-border px-3 py-1.5 text-ui-sm hover:bg-surface-hover disabled:opacity-50"
                        disabled={busy || !host.online}
                        onClick={() => void testConnection(host)}
                      >
                        加密 RPC
                      </button>
                      <button
                        type="button"
                        className="rounded-lg border border-border px-3 py-1.5 text-ui-sm hover:bg-surface-hover disabled:opacity-50"
                        disabled={busy || !host.online}
                        onClick={() => void testWorkspace(host)}
                      >
                        工作区隧道
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
            {testResult ? (
              <pre className="max-h-64 overflow-auto rounded-lg bg-background p-3 text-ui-xs text-foreground-subtle">
                {testResult}
              </pre>
            ) : null}
          </section>
        ) : null}
      </div>
    </div>
  );
}
