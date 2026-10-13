import {
  MODE_AGENT_PROVIDER,
  MODE_AGENT_PROVIDER_LABEL,
  type ModeProvider,
} from "@zcode/shared";

export const ZCODE_AGENT_PROVIDERS: ModeProvider[] = [MODE_AGENT_PROVIDER];

export const ZCODE_AGENT_PROVIDER_LABELS: Record<ModeProvider, string> = {
  glm: MODE_AGENT_PROVIDER_LABEL,
};

export function supportsZCodeSessionFork(provider: ModeProvider): boolean {
  void provider;
  // 功能开关：fork 入口恢复对所有 ZCode provider 开放；实际可操作性仍由 task 状态、
  // assistant 目标和运行中保护 guard 控制，避免把 active/failed 分支暴露为可 fork。
  return true;
}

export function shouldHideZCodeSessionForkAction(provider: ModeProvider): boolean {
  return !supportsZCodeSessionFork(provider);
}
