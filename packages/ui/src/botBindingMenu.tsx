import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ComponentType,
  type ReactNode,
} from "react";
import {
  getBotWorkspaceKey,
  isBotEligibleForSessionBinding,
  type BotConfig,
} from "@mode/shared";
import type { BotConversationSummary } from "@mode/services";
import {
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu.js";
import {
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu.js";
import { useServices } from "@/hooks/useServices.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import { toast } from "@/components/ui/toast.js";
import {
  useBotTaskBinding,
  refreshBotTaskBindings,
  type BotTaskBinding,
} from "@/store/botTaskBindingsStore.js";

/** 名称可能重复或为空，bot ID 才是唯一身份：每一项都把 ID 一并显示出来。 */
function BotBindingMenuItemContent({ label, botId }: { label: string; botId: string }): ReactNode {
  return (
    <span className="flex w-full min-w-0 items-center gap-3">
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span
        className="max-w-[9rem] shrink-0 truncate font-mono text-ui-sm text-foreground-subtle"
        title={botId}
      >
        {botId}
      </span>
    </span>
  );
}

/**
 * 菜单原语插槽：同一份「IM 机器人」子菜单既要挂进任务行右键菜单（ContextMenu），
 * 也要挂进工作区标题栏的「···」下拉（DropdownMenu）。Radix 两套原语结构一致，
 * 这里按插槽注入，避免复制一份业务逻辑。
 */
export interface BotBindingMenuPrimitives {
  Sub: ComponentType<{ children: ReactNode }>;
  SubTrigger: ComponentType<{ disabled?: boolean; children: ReactNode }>;
  SubContent: ComponentType<{ className?: string; children: ReactNode }>;
  Item: ComponentType<{
    disabled?: boolean;
    onSelect?: () => void;
    className?: string;
    children: ReactNode;
  }>;
}

const CONTEXT_MENU_PRIMITIVES: BotBindingMenuPrimitives = {
  Sub: ContextMenuSub,
  SubTrigger: ContextMenuSubTrigger,
  SubContent: ContextMenuSubContent,
  Item: ContextMenuItem,
};

/** 同一份子菜单挂进 DropdownMenu（工作区标题栏「···」）时用的原语。 */
export const DROPDOWN_MENU_PRIMITIVES: BotBindingMenuPrimitives = {
  Sub: DropdownMenuSub,
  SubTrigger: DropdownMenuSubTrigger,
  SubContent: DropdownMenuSubContent,
  Item: DropdownMenuItem,
};

/**
 * 会话「IM 机器人」子菜单：把任务绑定到某个机器人（对齐 MyAgents 的 /task set 语义的桌面入口）。
 * 绑定后：IM 消息进入这个会话、桌面与 IM 双向实时同步（用户输入与助手回复都镜像）。
 * 只列出属于当前工作区的机器人（工作区绑定的那个，或显式授权本工作区的 bot）——服务端同样强制。
 * 数据在菜单打开时才拉取（菜单本身是按需挂载的单例，避免每个任务行常驻订阅）。
 */
export function BotBindingMenuItems({
  task,
  primitives = CONTEXT_MENU_PRIMITIVES,
}: {
  task: { taskId: string; workspacePath: string; workspaceIdentity?: string };
  primitives?: BotBindingMenuPrimitives;
}): ReactNode {
  const { Sub, SubTrigger, SubContent, Item } = primitives;
  const { intl } = useZCodeIntl();
  const services = useServices();
  const taskBindings = useBotTaskBinding(task.workspacePath, task.workspaceIdentity, task.taskId);
  const boundBotIds = useMemo(() => taskBindings.map((binding) => binding.botId), [taskBindings]);
  const [bots, setBots] = useState<BotConfig[] | null>(null);
  const [workspaceBoundBotIds, setWorkspaceBoundBotIds] = useState<string[]>([]);
  // 每个 bot 的候选对话（含"还没说过话但可预判"的私聊/群）：绑定按对话粒度，先选对话。
  const [conversationsByBot, setConversationsByBot] = useState<
    Record<string, BotConversationSummary[]>
  >({});
  const [busy, setBusy] = useState(false);
  const workspaceKey = getBotWorkspaceKey(task.workspacePath, task.workspaceIdentity);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      services.botsService.getConfig(),
      services.botsService.getWorkspaceBotBinding({
        workspacePath: task.workspacePath,
        ...(task.workspaceIdentity ? { workspaceIdentity: task.workspaceIdentity } : {}),
      }),
    ])
      .then(async ([config, binding]) => {
        if (cancelled) {
          return;
        }
        setBots(config.bots);
        setWorkspaceBoundBotIds(binding.botIds);
        const conversationEntries = await Promise.all(
          config.bots.map(async (bot) => {
            const conversations = await services.botsService
              .listBotConversations({ botId: bot.id })
              .catch(() => []);
            return [bot.id, conversations] as const;
          }),
        );
        if (!cancelled) {
          setConversationsByBot(Object.fromEntries(conversationEntries));
        }
      })
      .catch((error: unknown) => {
        logger.warn(
          "[BotBindingMenu] load bots failed",
          error instanceof Error ? error.message : String(error),
        );
        if (!cancelled) {
          setBots([]);
          setWorkspaceBoundBotIds([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [services.botsService, task.workspaceIdentity, task.workspacePath]);

  const refreshBindings = useCallback(async () => {
    try {
      await refreshBotTaskBindings(services.botsService);
    } catch (error) {
      logger.warn(
        "[BotBindingMenu] refresh bindings failed",
        error instanceof Error ? error.message : String(error),
      );
    }
  }, [services.botsService]);

  const displayName = useCallback(
    (bot: BotConfig) =>
      bot.name.trim() || intl.formatMessage({ id: `bots.channel.${bot.provider}` }),
    [intl],
  );

  /** 对话展示名：私聊带昵称/ID，群聊带群 ID。 */
  const describeConversation = useCallback(
    (conversation: {
      conversationKind: "private" | "group";
      conversationId: string;
      conversationLabel?: string;
    }) =>
      conversation.conversationKind === "group"
        ? intl.formatMessage(
            { id: "taskList.botBind.conversation.group" },
            { id: conversation.conversationId },
          )
        : intl.formatMessage(
            { id: "taskList.botBind.conversation.private" },
            { name: conversation.conversationLabel?.trim() || conversation.conversationId },
          ),
    [intl],
  );

  const handleBind = useCallback(
    async (bot: BotConfig, conversation?: BotConversationSummary) => {
      setBusy(true);
      try {
        const result = await services.botsService.bindBotToTask({
          botId: bot.id,
          ...(conversation ? { conversationKey: conversation.conversationKey } : {}),
          workspacePath: task.workspacePath,
          ...(task.workspaceIdentity ? { workspaceIdentity: task.workspaceIdentity } : {}),
          taskId: task.taskId,
        });
        if (!result.ok) {
          toast(
            intl.formatMessage({
              id:
                result.reason === "busy"
                  ? "taskList.botBind.busy"
                  : result.reason === "workspace"
                    ? "taskList.botBind.notInWorkspace"
                    : result.reason === "conversation"
                      ? "taskList.botBind.conversationUnavailable"
                      : "taskList.botBind.failed",
            }),
          );
          return;
        }
        toast(
          intl.formatMessage(
            { id: "taskList.botBind.boundToast" },
            {
              name: displayName(bot),
              botId: bot.id,
              ...(conversation
                ? { conversation: describeConversation(conversation) }
                : {}),
            },
          ),
        );
        await refreshBindings();
      } finally {
        setBusy(false);
      }
    },
    [
      describeConversation,
      displayName,
      intl,
      refreshBindings,
      services.botsService,
      task.taskId,
      task.workspaceIdentity,
      task.workspacePath,
    ],
  );

  const handleUnbind = useCallback(
    async (bot: BotConfig, binding: BotTaskBinding) => {
      setBusy(true);
      try {
        await services.botsService.unbindBotFromTask({
          botId: bot.id,
          taskId: task.taskId,
          conversationKey: binding.conversationKey,
        });
        toast(
          intl.formatMessage(
            { id: "taskList.botBind.unboundToast" },
            { name: displayName(bot), botId: bot.id, conversation: describeConversation(binding) },
          ),
        );
        await refreshBindings();
      } finally {
        setBusy(false);
      }
    },
    [describeConversation, displayName, intl, refreshBindings, services.botsService, task.taskId],
  );

  const allBots = bots ?? [];
  const eligibleBots = allBots.filter(
    (bot) =>
      bot.enabled &&
      isBotEligibleForSessionBinding({ bot, workspaceKey, workspaceBoundBotIds }),
  );
  // 已经不满足资格的存量绑定（例如机器人被移出本工作区）：仍要给出解绑入口，否则用户被卡住。
  const extraUnbindBots = allBots.filter(
    (bot) =>
      boundBotIds.includes(bot.id) &&
      !eligibleBots.some((eligible) => eligible.id === bot.id),
  );
  // 同一 bot 可以有多条绑定（不同对话各自绑到本会话）；解绑按对话粒度给出入口。
  const bindingsByBot = useMemo(() => {
    const map = new Map<string, BotTaskBinding[]>();
    for (const binding of taskBindings) {
      const list = map.get(binding.botId) ?? [];
      list.push(binding);
      map.set(binding.botId, list);
    }
    return map;
  }, [taskBindings]);
  const isEmpty = eligibleBots.length === 0 && extraUnbindBots.length === 0;

  return (
    <Sub>
      <SubTrigger disabled={busy}>
        {intl.formatMessage({ id: "taskList.botBind.menu" })}
      </SubTrigger>
      <SubContent className="w-80">
        {bots === null ? (
          <Item disabled>{intl.formatMessage({ id: "taskList.botBind.loading" })}</Item>
        ) : isEmpty ? (
          <>
            <Item disabled>
              {intl.formatMessage({ id: "taskList.botBind.emptyWorkspace" })}
            </Item>
            <Item disabled className="text-ui-sm">
              {intl.formatMessage({ id: "taskList.botBind.emptyWorkspaceHint" })}
            </Item>
          </>
        ) : (
          <>
            {eligibleBots.map((bot) => {
              const botBindings = bindingsByBot.get(bot.id) ?? [];
              const unboundConversations = (conversationsByBot[bot.id] ?? []).filter(
                (conversation) =>
                  !botBindings.some(
                    (binding) => binding.conversationKey === conversation.conversationKey,
                  ),
              );
              return (
                <Fragment key={bot.id}>
                  {botBindings.map((binding) => (
                    <Item
                      key={`unbind:${binding.conversationKey}`}
                      disabled={busy}
                      onSelect={() => {
                        void handleUnbind(bot, binding);
                      }}
                    >
                      <BotBindingMenuItemContent
                        label={intl.formatMessage(
                          { id: "taskList.botBind.unbind" },
                          {
                            name: displayName(bot),
                            conversation: describeConversation(binding),
                          },
                        )}
                        botId={bot.id}
                      />
                    </Item>
                  ))}
                  {unboundConversations.length === 1 ? (
                    <Item
                      disabled={busy}
                      onSelect={() => {
                        void handleBind(bot, unboundConversations[0]!);
                      }}
                    >
                      <BotBindingMenuItemContent
                        label={intl.formatMessage(
                          { id: "taskList.botBind.bind" },
                          {
                            name: displayName(bot),
                            conversation: describeConversation(unboundConversations[0]!),
                          },
                        )}
                        botId={bot.id}
                      />
                    </Item>
                  ) : unboundConversations.length === 0 ? null : (
                    // 多个候选对话：先选对话再绑定（一个 bot 可同时服务多个对话）。
                    <Sub key={`bind:${bot.id}`}>
                      <SubTrigger disabled={busy}>
                        {intl.formatMessage(
                          { id: "taskList.botBind.chooseConversation" },
                          { name: displayName(bot) },
                        )}
                      </SubTrigger>
                      <SubContent className="w-72">
                        {unboundConversations.map((conversation) => (
                          <Item
                            key={conversation.conversationKey}
                            disabled={busy}
                            onSelect={() => {
                              void handleBind(bot, conversation);
                            }}
                          >
                            <BotBindingMenuItemContent
                              label={describeConversation(conversation)}
                              botId={conversation.conversationId}
                            />
                          </Item>
                        ))}
                      </SubContent>
                    </Sub>
                  )}
                </Fragment>
              );
            })}
            {extraUnbindBots.map((bot) =>
              (bindingsByBot.get(bot.id) ?? []).map((binding) => (
                <Item
                  key={`${bot.id}:${binding.conversationKey}`}
                  disabled={busy}
                  onSelect={() => {
                    void handleUnbind(bot, binding);
                  }}
                >
                  <BotBindingMenuItemContent
                    label={intl.formatMessage(
                      { id: "taskList.botBind.unbind" },
                      { name: displayName(bot), conversation: describeConversation(binding) },
                    )}
                    botId={bot.id}
                  />
                </Item>
              )),
            )}
          </>
        )}
      </SubContent>
    </Sub>
  );
}
