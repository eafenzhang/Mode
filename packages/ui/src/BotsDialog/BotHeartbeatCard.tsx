import { useState } from "react";
import type { BotConfig } from "@mode/shared";
import {
  BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MIN_INTERVAL_MINUTES,
} from "@mode/shared";
import { Input } from "@/components/ui/input.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 机器人心跳：定时唤醒 agent 检查工作区。
 * 输入框本身就是开关——0 表示关闭（默认），15–1440 表示间隔分钟数；
 * 有内容才汇报（HEARTBEAT_OK 回执会被抑制），需要更细的指引时由工作区里的 HEARTBEAT.md 承担。
 */
export function BotHeartbeatCard({
  bot,
  onPatchBot,
}: {
  bot: BotConfig;
  onPatchBot: (patch: Partial<BotConfig>) => void;
}) {
  const { intl } = useZCodeIntl();
  const enabled = bot.heartbeat?.enabled === true;
  const intervalMinutes = bot.heartbeat?.intervalMinutes ?? BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES;
  // 关闭时显示 0；上一次的间隔保留在配置里，重新填正数即可恢复同一节奏。
  const displayMinutes = enabled ? intervalMinutes : 0;
  const [intervalDraft, setIntervalDraft] = useState<string | null>(null);
  const draftValue = intervalDraft ?? String(displayMinutes);

  const commitInterval = () => {
    const parsed = Number.parseInt(draftValue, 10);
    setIntervalDraft(null);
    if (!Number.isFinite(parsed) || parsed < 0) {
      return;
    }
    if (parsed === 0) {
      if (enabled) {
        onPatchBot({ heartbeat: { enabled: false, intervalMinutes } });
      }
      return;
    }
    const clamped = Math.min(
      BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
      Math.max(BOT_HEARTBEAT_MIN_INTERVAL_MINUTES, parsed),
    );
    if (enabled && clamped === intervalMinutes) {
      return;
    }
    onPatchBot({ heartbeat: { enabled: true, intervalMinutes: clamped } });
  };

  return (
    <SettingsRow
      label={intl.formatMessage({ id: "bots.heartbeat.title" })}
      description={intl.formatMessage(
        { id: "bots.heartbeat.description" },
        {
          min: String(BOT_HEARTBEAT_MIN_INTERVAL_MINUTES),
          max: String(BOT_HEARTBEAT_MAX_INTERVAL_MINUTES),
        },
      )}
      control={
        <div className="relative w-48">
          <Input
            size="lg"
            inputMode="numeric"
            value={draftValue}
            onChange={(event) => setIntervalDraft(event.target.value.replace(/[^\d]/gu, ""))}
            onBlur={commitInterval}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.currentTarget.blur();
              }
            }}
            className="w-full pe-11 text-end"
            aria-label={intl.formatMessage({ id: "bots.heartbeat.intervalLabel" })}
          />
          <span className="pointer-events-none absolute end-3 top-1/2 -translate-y-1/2 whitespace-nowrap text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "bots.heartbeat.intervalSuffix" })}
          </span>
        </div>
      }
    />
  );
}
