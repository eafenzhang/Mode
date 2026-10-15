/**
 * 设备卡上的「连接选项」弹出面板（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 9）。
 *
 * 全部选项按设备记忆：读 `rdGetRdpOptions`、确认时由设备页 `rdSetRdpOptions` 持久化后再
 * `rdConnect`。本组件只编辑草稿（受控），不直接落盘——写路径只有设备页一处。
 * 端口用本地字符串态编辑：允许中途清空重输，非法/越界值在确认前不写回（不静默截断）。
 */
import { useState } from "react";
import { Checkbox } from "@/components/ui/checkbox.js";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import type { RdpAudioMode } from "@mode/shared/remote-desktop-rdp";
import {
  RDP_AUDIO_MODES,
  RDP_OPTION_DEFAULTS,
  type ResolvedRdpOptions,
} from "@/remote-desktop/rdpOptions.js";

const AUDIO_LABEL_IDS: Record<RdpAudioMode, string> = {
  local: "remoteDesktop.options.audio.local",
  remote: "remoteDesktop.options.audio.remote",
  disabled: "remoteDesktop.options.audio.disabled",
};

function OptionCheckbox({
  testId,
  labelId,
  checked,
  disabled,
  onCheckedChange,
}: {
  testId: string;
  labelId: string;
  checked: boolean;
  disabled: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const { intl } = useModeIntl();
  return (
    <label className="flex cursor-pointer items-center gap-2 text-ui-base text-foreground">
      <Checkbox
        data-testid={testId}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(value) => onCheckedChange(value === true)}
      />
      {intl.formatMessage({ id: labelId })}
    </label>
  );
}

export function RemoteDesktopConnectOptions({
  options,
  busy,
  onChange,
  onConfirm,
  onCancel,
}: {
  options: ResolvedRdpOptions;
  busy: boolean;
  onChange: (next: ResolvedRdpOptions) => void;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { intl } = useModeIntl();
  // 端口编辑态与数值态分离：字符串允许中间态，数值只接受 1..65535。
  const [portText, setPortText] = useState(() => String(options.port));

  const portValue = Number(portText);
  const portValid =
    portText.trim() !== "" &&
    Number.isInteger(portValue) &&
    portValue >= 1 &&
    portValue <= 65535;

  const commitPort = (text: string) => {
    setPortText(text);
    const next = Number(text);
    if (text.trim() !== "" && Number.isInteger(next) && next >= 1 && next <= 65535) {
      onChange({ ...options, port: next });
    }
  };

  return (
    <div className="w-72 space-y-3" data-testid="remote-desktop-options-panel">
      <span className="block text-ui-base font-medium text-foreground">
        {intl.formatMessage({ id: "remoteDesktop.options.title" })}
      </span>

      <div className="space-y-2">
        <OptionCheckbox
          testId="remote-desktop-option-fullscreen"
          labelId="remoteDesktop.options.fullscreen"
          checked={options.fullscreen}
          disabled={busy}
          onCheckedChange={(checked) => onChange({ ...options, fullscreen: checked })}
        />
        <OptionCheckbox
          testId="remote-desktop-option-multimon"
          labelId="remoteDesktop.options.multimon"
          checked={options.multimon}
          disabled={busy}
          onCheckedChange={(checked) => onChange({ ...options, multimon: checked })}
        />
        <OptionCheckbox
          testId="remote-desktop-option-clipboard"
          labelId="remoteDesktop.options.clipboard"
          checked={options.clipboard}
          disabled={busy}
          onCheckedChange={(checked) => onChange({ ...options, clipboard: checked })}
        />
      </div>

      <div className="space-y-1">
        <span className="block text-ui-xs text-foreground-subtle">
          {intl.formatMessage({ id: "remoteDesktop.options.audio.label" })}
        </span>
        <Select
          value={options.audio}
          disabled={busy}
          onValueChange={(value) => onChange({ ...options, audio: value as RdpAudioMode })}
        >
          <SelectTrigger size="lg" className="w-full min-w-0 justify-between">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RDP_AUDIO_MODES.map((mode) => (
              <SelectItem key={mode} value={mode}>
                {intl.formatMessage({ id: AUDIO_LABEL_IDS[mode] })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <label className="block space-y-1">
        <span className="block text-ui-xs text-foreground-subtle">
          {intl.formatMessage({ id: "remoteDesktop.options.username.label" })}
        </span>
        <Input
          size="lg"
          data-testid="remote-desktop-option-username"
          value={options.username}
          disabled={busy}
          placeholder={intl.formatMessage({ id: "remoteDesktop.options.username.placeholder" })}
          onChange={(event) => onChange({ ...options, username: event.target.value })}
        />
        <span className="block text-ui-xs text-foreground-subtlest">
          {intl.formatMessage({ id: "remoteDesktop.options.username.hint" })}
        </span>
      </label>

      <label className="block space-y-1">
        <span className="block text-ui-xs text-foreground-subtle">
          {intl.formatMessage({ id: "remoteDesktop.options.port.label" })}
        </span>
        <Input
          size="lg"
          inputMode="numeric"
          data-testid="remote-desktop-option-port"
          value={portText}
          disabled={busy}
          placeholder={String(RDP_OPTION_DEFAULTS.port)}
          onChange={(event) => commitPort(event.target.value.replace(/[^0-9]/gu, "").slice(0, 5))}
        />
        {portValid ? null : (
          <span className="block text-ui-xs text-warning">
            {intl.formatMessage({ id: "remoteDesktop.options.port.invalid" })}
          </span>
        )}
      </label>

      <div className="flex justify-end gap-2 pt-1">
        <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
        </Button>
        <Button
          type="button"
          size="sm"
          data-testid="remote-desktop-options-confirm"
          disabled={busy || !portValid}
          onClick={onConfirm}
        >
          {intl.formatMessage({ id: "remoteDesktop.options.confirm" })}
        </Button>
      </div>
    </div>
  );
}
