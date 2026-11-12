import { useEffect, type Dispatch, type SetStateAction } from "react";
import type { WebAccountClient, WebDeviceProfile, WebGithubChallenge } from "@mode/remote-core/web";

/** Device Flow 回退：拿到验证码后按服务端间隔轮询，授权成功写回 profile。 */
export function useV3DeviceLogin(
  challenge: WebGithubChallenge | null,
  client: WebAccountClient | null,
  setProfile: Dispatch<SetStateAction<WebDeviceProfile | null>>,
  refreshHosts: () => Promise<void>,
  setChallenge: Dispatch<SetStateAction<WebGithubChallenge | null>>,
  setError: Dispatch<SetStateAction<string>>,
): void {
  useEffect(() => {
    if (!challenge || challenge.kind !== "device" || !client) return;
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
        if (result.status === "error") {
          setError(result.message);
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
  }, [challenge, client, refreshHosts, setChallenge, setError, setProfile]);
}
