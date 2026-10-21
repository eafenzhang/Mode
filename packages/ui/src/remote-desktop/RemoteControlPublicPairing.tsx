import { useCallback, useState } from "react";
import { Globe, LoaderIcon } from "lucide-react";
import { parseLanPublicBaseUrl } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";

/**
 * 公网配对入口（docs/specs/lan-remote-public-endpoints.md 产品规则 5）：
 * 无法直连 LAN 时，用已配置 HTTPS 隧道的域名 + 对端 6 位码完成首次配对。
 * 只接受 https，避免把令牌/输入暴露在明文公网链路。
 */
export function RemoteControlPublicPairing({ onPaired }: { onPaired: () => void | Promise<void> }) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [error, setError] = useState("");

  const pair = useCallback(async () => {
    const endpoint = parseLanPublicBaseUrl(address);
    if (!endpoint) {
      setError(intl.formatMessage({ id: "remoteDesktop.publicPairing.invalidAddress" }));
      return;
    }
    if (!code.trim()) {
      setError(intl.formatMessage({ id: "remoteDesktop.publicPairing.missingCode" }));
      return;
    }
    setPairing(true);
    setError("");
    try {
      await platform.pairLanPeer({
        host: endpoint.host,
        port: endpoint.port,
        code: code.trim(),
        scheme: "https",
        ...(endpoint.path ? { path: endpoint.path } : {}),
      });
      setAddress("");
      setCode("");
      await onPaired();
    } catch (caught) {
      setError(
        `${intl.formatMessage({ id: "remoteDesktop.publicPairing.failed" })}：${
          caught instanceof Error ? caught.message : String(caught)
        }`,
      );
    } finally {
      setPairing(false);
    }
  }, [address, code, intl, onPaired, platform]);

  return (
    <section
      className="rounded-xl border border-border bg-card px-4 py-3"
      data-testid="remote-control-public-pairing"
    >
      <div className="flex items-center gap-2">
        <Globe className="size-4 text-foreground-subtle" aria-hidden="true" />
        <h2 className="text-ui-lg font-medium text-foreground">
          {intl.formatMessage({ id: "remoteDesktop.publicPairing.title" })}
        </h2>
      </div>
      <p className="mt-1 text-ui-base text-foreground-subtle">
        {intl.formatMessage({ id: "remoteDesktop.publicPairing.description" })}
      </p>
      <div className="mt-3 grid gap-3 sm:grid-cols-[minmax(0,1fr)_10rem_auto]">
        <div>
          <label className="mb-1 block text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.publicPairing.addressLabel" })}
          </label>
          <Input
            size="lg"
            data-testid="remote-control-public-address"
            value={address}
            placeholder={intl.formatMessage({
              id: "remoteDesktop.publicPairing.addressPlaceholder",
            })}
            onChange={(event) => {
              setAddress(event.target.value);
              setError("");
            }}
          />
        </div>
        <div>
          <label className="mb-1 block text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.publicPairing.codeLabel" })}
          </label>
          <Input
            size="lg"
            data-testid="remote-control-public-code"
            className="tracking-[0.3em] uppercase"
            value={code}
            maxLength={8}
            onChange={(event) => {
              setCode(event.target.value);
              setError("");
            }}
          />
        </div>
        <div className="flex items-end">
          <Button
            type="button"
            variant="outline"
            size="lg"
            data-testid="remote-control-public-pair"
            disabled={pairing}
            onClick={() => void pair()}
          >
            {pairing ? (
              <LoaderIcon className="size-4 animate-spin" />
            ) : (
              <Globe className="size-4" />
            )}
            {intl.formatMessage({
              id: pairing
                ? "remoteDesktop.publicPairing.pairing"
                : "remoteDesktop.publicPairing.pair",
            })}
          </Button>
        </div>
      </div>
      {error ? (
        <p className="mt-2 text-ui-base text-destructive" data-testid="remote-control-public-error">
          {error}
        </p>
      ) : (
        <p className="mt-2 text-ui-xs text-foreground-subtlest">
          {intl.formatMessage({ id: "remoteDesktop.publicPairing.hint" })}
        </p>
      )}
    </section>
  );
}
