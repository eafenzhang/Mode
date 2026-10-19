/**
 * 被控指示条（LR-3，renderer 覆盖层）。
 *
 * Electron 线由 main 的 `indicatorWindow` 显示「正在被控制」；Tauri 没有 main，指示条由
 * renderer 渲染。它订阅 Host 的被控状态（同意等待或会话进行中都算），显示设备名与「结束」。
 *
 * 已知边界：renderer 覆盖层只在应用窗口可见时可见；壳 `WebviewWindow` 是后续产品决策。
 */
import { useCallback, useEffect, useState } from "react";
import { MonitorPlay, X } from "lucide-react";

import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";

export function RemoteDesktopIndicator() {
  const platform = usePlatform();
  const { intl } = useModeIntl();
  const [controlled, setControlled] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 被控方设备名（Host 同意请求里带 label；状态事件只有布尔值，所以单独记）。 */
  const [deviceLabel, setDeviceLabel] = useState<string | null>(null);

  useEffect(() => {
    const subscribe = platform.onRemoteDesktopControlledStateChanged;
    if (!subscribe) {
      return;
    }
    return subscribe((state) => {
      setControlled(state.controlled);
      if (!state.controlled) {
        setDeviceLabel(null);
      }
    });
  }, [platform]);

  useEffect(() => {
    const subscribe = platform.onRemoteDesktopConsentRequest;
    if (!subscribe) {
      return;
    }
    // 同意请求到达时记下设备名；拒绝/超时后 controlled 变 false 会清掉。
    return subscribe((request) => setDeviceLabel(request.label || request.clientId));
  }, [platform]);

  const endSession = useCallback(async () => {
    if (busy) {
      return;
    }
    setBusy(true);
    try {
      // 关掉被控总开关会终止进行中的会话（与设置页同一写路径）。
      await platform.rdSetServerEnabled(false);
    } catch (error) {
      logger.warn("[remote-desktop] 结束被控会话失败", error);
    } finally {
      setBusy(false);
    }
  }, [busy, platform]);

  if (!controlled) {
    return null;
  }

  return (
    <div
      className="pointer-events-auto fixed bottom-4 right-4 z-50 flex items-center gap-3 rounded-lg border border-border bg-surface-raised px-3 py-2 shadow-lg"
      role="status"
      aria-live="polite"
    >
      <MonitorPlay className="size-4 text-foreground-muted" aria-hidden />
      <span className="text-ui-sm text-foreground">
        {deviceLabel
          ? intl.formatMessage({ id: "remoteDesktop.indicator.text" }, { device: deviceLabel })
          : intl.formatMessage({ id: "remoteDesktop.indicator.controlling" })}
      </span>
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => void endSession()}
        aria-label={intl.formatMessage({ id: "remoteDesktop.indicator.end" })}
      >
        <X className="size-3.5" aria-hidden />
        {intl.formatMessage({ id: "remoteDesktop.indicator.end" })}
      </Button>
    </div>
  );
}
