import { useModeIntl } from "@/i18n/IntlProvider.js";
import { formatReplySpeed, type ReplySpeed } from "@/v4/conversationReplySpeedModel.js";

/** 轮尾动作行的 prefill/decode 速度；值未知时不渲染，避免显示 0 或占位符。 */
export function ConversationReplySpeed({ speed }: { speed?: ReplySpeed | null }) {
  const { locale } = useModeIntl();
  if (!speed) return null;
  return (
    <>
      {speed.prefillTokensPerSecond === null ? null : (
        <span
          data-testid="v4-speed-prefill"
          className="select-none text-ui-sm text-foreground-subtlest"
        >
          ↑{formatReplySpeed(speed.prefillTokensPerSecond, locale)} tok/s
        </span>
      )}
      {speed.decodeTokensPerSecond === null ? null : (
        <span
          data-testid="v4-speed-decode"
          className="select-none text-ui-sm text-foreground-subtlest"
        >
          ↓{formatReplySpeed(speed.decodeTokensPerSecond, locale)} tok/s
        </span>
      )}
    </>
  );
}
