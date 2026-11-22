//! 频道装配（LT-15 P2 切一）：把 inprocess 壳的频道构造清单抽为可复用模块。
//!
//! 行为契约与 `apps/desktop-tauri/src-tauri/src/inprocess.rs` 的 `build_server`
//! 逐分支等价（搬移不改动）；两处参数化：
//! - 壳日志（`crate::host::shell_log`）改为 [`ChannelAssemblyOptions::on_shell_log`]
//!   注入；
//! - `AgentControlClient`（mode-agent/mcp-sync/agent-health/hooks 需要）改为
//!   [`ChannelAssemblyOptions::agent_control`]——`None` 时这四个频道不可构造，
//!   与 inprocess「not constructible」留痕同口径。
//!
//! `mode-agent` 的**注册与否**仍由调用方名单决定（S2 门禁，
//! spec docs/specs/host-ownership-switch.md §5）；本模块只提供构造。
//! server-mode（`crates/server`）用 [`register_service_channels`] 批量注册
//! 与 inprocess 默认集一致的频道（不含 mode-agent）。

use std::path::{PathBuf};
use std::sync::Arc;

use agent_process::AgentControlClient;
use rpc_host::{ChannelServer, ServerChannel};

use crate::attachment_transfer::AttachmentTransferChannel;
use crate::broadcast::BroadcastChannel;
use crate::client_metadata::{ClientConfigChannel, ClientScenesChannel};
use crate::commands::CommandsChannel;
use crate::conversation_mirror::ConversationMirrorChannel;
use crate::credential::CredentialChannel;
use crate::file::FileChannel;
use crate::file_watcher::FileWatcherChannel;
use crate::git::GitChannel;
use crate::git_checkpoint::GitCheckpointChannel;
use crate::health::AgentHealthChannel;
use crate::hooks::HooksChannel;
use crate::lan_access::LanAccessChannel;
use crate::media_preview::MediaPreviewChannel;
use crate::memory_channel::MemoryChannel;
use crate::mode_session::ModeSessionChannel;
use crate::mode_task::ModeTaskChannel;
use crate::onboarding::OnboardingRecordChannel;
use crate::mcp_sync::McpSyncChannel;
use crate::mode_agent::ModeAgentChannel;
use crate::plugin_management::PluginManagementChannel;
use crate::plugin_sync::PluginSyncChannel;
use crate::setting::SettingChannel;
use crate::settings_sync::SettingsSyncChannel;
use crate::skill_sync::SkillSyncChannel;
use crate::skills::SkillsChannel;
use crate::subagents::SubagentsChannel;
use crate::system::SystemChannel;
use crate::terminal::TerminalChannel;
use crate::usage_stats::UsageStatsChannel;
use crate::window_controller::WindowControllerChannel;
use crate::{
    AGENT_HEALTH_CHANNEL, ATTACHMENT_TRANSFER_CHANNEL, BROADCAST_CHANNEL, CLIENT_CONFIG_CHANNEL,
    CLIENT_SCENES_CHANNEL, COMMANDS_CHANNEL, CONVERSATION_MIRROR_CHANNEL, CREDENTIAL_CHANNEL,
    FILE_CHANNEL, FILE_WATCHER_CHANNEL, GIT_CHANNEL, GIT_CHECKPOINT_CHANNEL, HOOKS_CHANNEL,
    LAN_ACCESS_CHANNEL, MCP_SYNC_CHANNEL, MEDIA_PREVIEW_CHANNEL, MEMORY_CHANNEL,
    MODE_AGENT_CHANNEL,
    MODE_SESSION_CHANNEL, MODE_TASK_CHANNEL, ONBOARDING_RECORD_CHANNEL,
    PLUGIN_MANAGEMENT_CHANNEL, PLUGIN_SYNC_CHANNEL, SETTING_CHANNEL, SETTINGS_SYNC_CHANNEL,
    SKILLS_CHANNEL, SKILL_SYNC_CHANNEL, SUBAGENTS_CHANNEL, SYSTEM_CHANNEL, TERMINAL_CHANNEL,
    USAGE_STATS_CHANNEL, WINDOW_CONTROLLER_CHANNEL,
};

/// 壳侧结构化日志（降级/不可观察事件）；`None` 时静默。
pub type ShellLogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// 装配参数：壳与 server-mode 共用。
pub struct ChannelAssemblyOptions {
    /// 数据根（memory / conversation-mirror / git-checkpoint / plugin-sync / usage-stats）。
    pub data_root: PathBuf,
    /// Agent 控制面（mode-agent/mcp-sync/agent-health/hooks）；`None` 时不可构造。
    pub agent_control: Option<Arc<AgentControlClient>>,
    /// 壳侧结构化日志（降级/不可观察事件）；`None` 时静默。
    pub on_shell_log: Option<ShellLogSink>,
    /// 同一 server 内的任务事件总线：mode-task 发事件、window-controller 订阅刷新投影
    /// （TS Host 进程内 Emitter 等价）。每 server 一条，跨 server 不共享。
    pub task_events: Arc<crate::mode_task::TaskEventBus>,
    /// LAN server supervisor（桌面/服务端宿主注入）；`None` 时 lan-access 管理面显式报错。
    pub lan_host: Option<Arc<dyn crate::lan_access::LanAccessHostPort>>,
}

impl ChannelAssemblyOptions {
    fn log(&self, message: &str) {
        if let Some(on_shell_log) = &self.on_shell_log {
            on_shell_log(message);
        }
    }
}

/// 单频道构造（与 inprocess `build_server` 各分支体逐行等价）。
///
/// 返回 `None` = 该频道在当前装配参数下不可构造（依赖缺失），调用方留痕跳过。
pub fn build_channel(name: &str, options: &ChannelAssemblyOptions) -> Option<Arc<dyn ServerChannel>> {
    let data_root: &PathBuf = &options.data_root;
    let log = |message: &str| options.log(message);
    match name {
        MEMORY_CHANNEL => Some(Arc::new(MemoryChannel::new(Arc::new(
            project_memory::ProjectMemoryService::new(data_root.clone()),
        )))),
        BROADCAST_CHANNEL => Some(Arc::new(BroadcastChannel::new(Arc::new(
            broadcast_service::BroadcastService::new(),
        )))),
        ATTACHMENT_TRANSFER_CHANNEL => Some(Arc::new(AttachmentTransferChannel)),
        CLIENT_CONFIG_CHANNEL => Some(Arc::new(ClientConfigChannel)),
        CLIENT_SCENES_CHANNEL => Some(Arc::new(ClientScenesChannel)),
        MEDIA_PREVIEW_CHANNEL => Some(Arc::new(MediaPreviewChannel)),
        CONVERSATION_MIRROR_CHANNEL => Some(Arc::new(ConversationMirrorChannel::new(
            conversation_mirror::ConversationMirrorService::new(data_root.clone()),
        ))),
        SETTING_CHANNEL => {
            let env = settings_store::HomeEnv::from_process();
            let paths = settings_store::resolve_settings_paths(&env);
            let home = paths.home.clone();
            let current_base = data_root
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| home.clone());
            Some(Arc::new(SettingChannel::new(
                Arc::new(settings_store::SettingsService::new(paths, current_base)),
                home,
            )))
        }
        CREDENTIAL_CHANNEL => {
            // 坏凭据文件必须留下可观察的告警：服务层日志走不了 Node 的 logger，
            // 用壳侧结构化日志记录路径与备份名（不记录任何值）。
            let on_shell_log = options.on_shell_log.clone();
            let store = credential_store::CredentialStore::new(data_root.clone())
                .with_corrupt_listener(Arc::new(
                    move |path: &std::path::Path, backup: Option<&std::path::Path>| {
                        if let Some(on_shell_log) = &on_shell_log {
                            on_shell_log(&format!(
                                "credential store corrupt: path={} backup={}",
                                path.display(),
                                backup
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "<none>".to_string())
                            ));
                        }
                    },
                ));
            Some(Arc::new(CredentialChannel::new(Arc::new(store))))
        }
        SYSTEM_CHANNEL => Some(Arc::new(SystemChannel::new(Arc::new(
            system_service::SystemService::new(),
        )))),
        LAN_ACCESS_CHANNEL => {
            // 16.4a：对端配对 registry + 宿主注入的 supervisor（监听生命周期/
            // 配对码/clients/onDidChange）；未注入时 getState 走 offline 快照、
            // 管理面显式报错。
            let credentials = Arc::new(credential_store::CredentialStore::new(data_root.clone()));
            Some(match &options.lan_host {
                Some(host) => {
                    Arc::new(LanAccessChannel::with_host(credentials, Arc::clone(host)))
                }
                None => Arc::new(LanAccessChannel::new(credentials)),
            })
        }
        FILE_CHANNEL => {
            // 索引降级（规则文件不可用/解析失败）必须可观察；日志只带原因，不带文件内容。
            let on_shell_log = options.on_shell_log.clone();
            let mut options =
                file_service::FileServiceOptions::from_process(data_root.clone());
            options.on_warn = Some(Arc::new(move |message: &str| {
                if let Some(on_shell_log) = &on_shell_log {
                    on_shell_log(message);
                }
            }));
            Some(Arc::new(FileChannel::new(Arc::new(
                file_service::FileService::new(options),
            ))))
        }
        FILE_WATCHER_CHANNEL => {
            let on_shell_log = options.on_shell_log.clone();
            let service = file_watcher::FileWatcherService::with_warn_sink(Some(Arc::new(
                move |message: &str| {
                    if let Some(on_shell_log) = &on_shell_log {
                        on_shell_log(message);
                    }
                },
            )));
            Some(Arc::new(FileWatcherChannel::new(Arc::new(service))))
        }
        COMMANDS_CHANNEL => {
            // 命令域的真源以**用户 home** 为根（`{home}/.mode/commands`、
            // `{home}/.mode/cli/config.json`），自定义数据目录（`data_root`）不参与，
            // 与 TS `resolveUserHomeDir()` 一致；因此这里不传 `data_root`。
            Some(Arc::new(CommandsChannel::new(Arc::new(
                commands_service::CommandsService::new(
                    commands_service::CommandsServiceOptions::from_process(),
                ),
            ))))
        }
        SKILLS_CHANNEL => {
            // 用户级技能在桌面运行时始终可用：Node Host 显式传 `{ isDesktopRuntime: true }`，
            // 壳内没有 `MODE_PROCESS_LABEL`，不能依赖 `from_process()` 的默认判定。
            let mut options = skills_service::SkillsServiceOptions::from_process();
            options.is_desktop_runtime = true;
            Some(Arc::new(SkillsChannel::new(Arc::new(
                skills_service::SkillsService::new(options),
            ))))
        }
        SUBAGENTS_CHANNEL => {
            let mut options = subagents_service::SubagentsServiceOptions::from_process();
            options.is_desktop_runtime = Some(true);
            Some(Arc::new(SubagentsChannel::new(Arc::new(
                subagents_service::SubagentsService::new(options),
            ))))
        }
        SETTINGS_SYNC_CHANNEL => {
            // 首启提示状态落在 `setting.json`：与 `setting` 频道用同一套路径解析
            // （坏文件重试与原子写都在 `settings_store` 内），只是各持一个无状态实例。
            let env = settings_store::HomeEnv::from_process();
            let paths = settings_store::resolve_settings_paths(&env);
            let current_base = data_root
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| paths.home.clone());
            let settings = Arc::new(settings_store::SettingsService::new(paths, current_base));
            Some(Arc::new(SettingsSyncChannel::new(Arc::new(
                settings_sync_service::SettingsSyncService::new(
                    settings_sync_service::SettingsSyncServiceOptions::default(),
                    settings,
                ),
            ))))
        }
        SKILL_SYNC_CHANNEL => Some(Arc::new(SkillSyncChannel::new(Arc::new(
            skill_sync_service::SkillSyncService::new(
                skill_sync_service::SkillSyncServiceOptions::default(),
            ),
        )))),
        GIT_CHANNEL => Some(Arc::new(GitChannel::new(crate::git::default_service()))),
        GIT_CHECKPOINT_CHANNEL => Some(Arc::new(GitCheckpointChannel::new(
            crate::git_checkpoint::default_service(data_root.clone()),
        ))),
        PLUGIN_SYNC_CHANNEL => Some(Arc::new(PluginSyncChannel::new(
            crate::plugin_sync::default_service(),
        ))),
        WINDOW_CONTROLLER_CHANNEL => {
            // window-controller（16.3）：local 单源列表/变更面 + V4 帧订阅；
            // 订阅同一 server 的任务事件总线，mode-task 写入后刷新投影推 delta。
            let repo = task_index_repo::TaskIndexRepo::new(
                data_root.join("v2").join("tasks-index.sqlite"),
            );
            Some(Arc::new(
                WindowControllerChannel::with_task_events(
                    Arc::new(repo),
                    Arc::clone(&options.task_events),
                )
                .with_control(options.agent_control.clone()),
            ))
        }
        MODE_TASK_CHANNEL => {
            // 任务索引 DB 与桌面壳同位（{data_root}/v2/tasks-index.sqlite）；
            // 首切只读 + 分组管理写面（16.1），编排面（16.2）未知方法显式报错。
            let repo = task_index_repo::TaskIndexRepo::new(
                data_root.join("v2").join("tasks-index.sqlite"),
            );
            // 命令队列（16.2b）：send executor 包装 control.send_prompt；
            // 无 agent 控制面时队列不装配（对应方法显式 unsupported）。
            let queue = options.agent_control.as_ref().map(|control| {
                let control = Arc::clone(control);
                Arc::new(crate::task_command_queue::TaskCommandQueue::new(
                    Arc::new(move |target, params| {
                        let control = Arc::clone(&control);
                        Box::pin(async move {
                            control
                                .send_prompt(&target, params)
                                .await
                                .map(|_result| ())
                                .map_err(|error| error.to_string())
                        })
                    }),
                    crate::task_command_queue::default_now_ms(),
                ))
            });
            Some(Arc::new(
                ModeTaskChannel::new(
                    Arc::new(repo),
                    options.agent_control.clone(),
                    queue,
                    Arc::clone(&options.task_events),
                )
                .with_data_root(data_root.clone()),
            ))
        }
        ONBOARDING_RECORD_CHANNEL => {
            // onboarding 记录是设备级数据（TS getAppConfigDir → dataRoot/v2）；
            // 官方账号体系已下线（assertOfficialServiceRemoved("account")），
            // TS loadUserId 同为空，这里保持同一口径。
            let repo = Arc::new(task_index_repo::TaskIndexRepo::new(
                data_root.join("v2").join("tasks-index.sqlite"),
            ));
            let service = onboarding_record_service::OnboardingRecordService::new(
                data_root.join("v2").join("onboarding-record.json"),
                Arc::new(HostOnboardingEnvironment { repo }),
                Arc::new(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_millis() as i64)
                        .unwrap_or(0)
                }),
            );
            Some(Arc::new(OnboardingRecordChannel::new(Arc::new(service))))
        }
        MODE_SESSION_CHANNEL => options.agent_control.as_ref().map(|control| {
            // renderer 只剩 readSession 走本频道（其余已切 mode-agent）；补回 Node 的
            // apiRetry 暂存与 Claude 导入历史修复（spec mode-session-channel.md）。
            let on_shell_log = options.on_shell_log.clone();
            Arc::new(
                ModeSessionChannel::new(Arc::clone(control), data_root.clone()).with_log(Arc::new(
                    move |message: &str| {
                        if let Some(on_shell_log) = &on_shell_log {
                            on_shell_log(message);
                        }
                    },
                )),
            ) as Arc<dyn ServerChannel>
        }),
        USAGE_STATS_CHANNEL => {
            // 余额查询凭据源：Personal 层仓库（1s 轮询 LKG）。路径显式 env 优先，
            // 回退 data_root/v2（与 agent_runtime 的 ProviderPaths fallback 同口径）。
            // 仓库不可用时不阻塞频道注册：退化为 default（一律 unsupported）。
            let personal_path = std::env::var(
                provider_config_repository::MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
            )
            .ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                data_root
                    .join("v2")
                    .join(provider_config_repository::PERSONAL_PROVIDER_CONFIG_FILE_NAME)
            });
            match provider_config_repository::PersonalConfigRepository::new(
                provider_config_repository::PersonalConfigRepositoryOptions {
                    file_path: personal_path,
                    import_legacy: None,
                    on_recovery: None,
                    on_polling_error: None,
                    polling_interval: provider_config_repository::PollingInterval::Millis(1_000),
                },
            ) {
                Ok(repository) => {
                    // 首次读取同时启动轮询线程（与 AccountFactsRuntime 同款）。
                    let repository = Arc::new(repository);
                    let _ = repository.read();
                    match UsageStatsChannel::with_personal_layer(Arc::clone(&repository)) {
                        Ok(channel) => {
                            let _repository = repository;
                            Some(Arc::new(channel))
                        }
                        Err(error) => {
                            log(&format!(
                                "usage-stats channel degraded to unsupported: {error}"
                            ));
                            let _repository = repository;
                            Some(Arc::new(UsageStatsChannel::default_channel()))
                        }
                    }
                }
                Err(error) => {
                    log(&format!(
                        "usage-stats personal config unavailable: {error}; channel degraded to unsupported"
                    ));
                    Some(Arc::new(UsageStatsChannel::default_channel()))
                }
            }
        }
        MCP_SYNC_CHANNEL => options.agent_control.as_ref().map(|control| {
            Arc::new(McpSyncChannel::from_process(Arc::clone(control)))
                as Arc<dyn ServerChannel>
        }),
        PLUGIN_MANAGEMENT_CHANNEL => options.agent_control.as_ref().map(|control| {
            // 设置页插件管理：同名转发给 mode-agent（插件事实源在 agent 进程），
            // 仅 overview 补 officialMarketplaceEnabled（spec plugin-management-channel.md）。
            let agent = Arc::new(ModeAgentChannel::new(
                Arc::clone(control),
                data_root.join(".mode").join("plugin-workspace"),
            ));
            Arc::new(PluginManagementChannel::new(agent)) as Arc<dyn ServerChannel>
        }),
        MODE_AGENT_CHANNEL => options.agent_control.as_ref().map(|control| {
            // S2（host-ownership-switch §5）：默认不注册，只有开关显式包含
            // mode-agent 时才接管；未迁移方法返回 unsupported_method。
            Arc::new(ModeAgentChannel::new(
                Arc::clone(control),
                data_root.join(".mode").join("plugin-workspace"),
            )) as Arc<dyn ServerChannel>
        }),
        AGENT_HEALTH_CHANNEL => options.agent_control.as_ref().map(|control| {
            Arc::new(AgentHealthChannel::new(Arc::clone(control)))
                as Arc<dyn ServerChannel>
        }),
        HOOKS_CHANNEL => options.agent_control.as_ref().map(|control| {
            Arc::new(HooksChannel::new(Arc::clone(control))) as Arc<dyn ServerChannel>
        }),
        TERMINAL_CHANNEL => {
            let env = settings_store::HomeEnv::from_process();
            let paths = settings_store::resolve_settings_paths(&env);
            let current_base = data_root
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| paths.home.clone());
            let settings = Arc::new(settings_store::SettingsService::new(paths, current_base));
            Some(Arc::new(TerminalChannel::new(
                crate::terminal::default_service(settings),
            )))
        }
        _ => None,
    }
}

/// 与 inprocess 默认集一致的批量注册（不含 `mode-agent`；S2 门禁由调用方持有）。
///
/// 不可构造的频道（依赖缺失）留痕跳过，不中断其余频道注册。
pub fn register_service_channels(server: &mut ChannelServer, options: &ChannelAssemblyOptions) {
    for name in service_channel_names() {
        match build_channel(name, options) {
            Some(channel) => server.register_channel(name, channel),
            None => options.log(&format!("service channel not constructible: {name}")),
        }
    }
}

/// 批量注册的频道名单（与 inprocess 白名单一致，减 `mode-agent`）。
pub fn service_channel_names() -> Vec<&'static str> {
    vec![
        MEMORY_CHANNEL,
        ATTACHMENT_TRANSFER_CHANNEL,
        CLIENT_CONFIG_CHANNEL,
        CLIENT_SCENES_CHANNEL,
        MEDIA_PREVIEW_CHANNEL,
        CONVERSATION_MIRROR_CHANNEL,
        SETTING_CHANNEL,
        CREDENTIAL_CHANNEL,
        SYSTEM_CHANNEL,
        LAN_ACCESS_CHANNEL,
        FILE_CHANNEL,
        FILE_WATCHER_CHANNEL,
        COMMANDS_CHANNEL,
        SKILLS_CHANNEL,
        SUBAGENTS_CHANNEL,
        SETTINGS_SYNC_CHANNEL,
        BROADCAST_CHANNEL,
        SKILL_SYNC_CHANNEL,
        GIT_CHANNEL,
        GIT_CHECKPOINT_CHANNEL,
        PLUGIN_SYNC_CHANNEL,
        MODE_TASK_CHANNEL,
        WINDOW_CONTROLLER_CHANNEL,
        USAGE_STATS_CHANNEL,
        MCP_SYNC_CHANNEL,
        AGENT_HEALTH_CHANNEL,
        HOOKS_CHANNEL,
        MODE_SESSION_CHANNEL,
        ONBOARDING_RECORD_CHANNEL,
        PLUGIN_MANAGEMENT_CHANNEL,
        TERMINAL_CHANNEL,
    ]
}

/// onboarding-record 的宿主环境：当前用户恒为匿名（官方账号已下线），
/// 「本机已有任务」读任务索引（TS `listTaskMetas({})` 非空判定）。
struct HostOnboardingEnvironment {
    repo: Arc<task_index_repo::TaskIndexRepo>,
}

impl onboarding_record_service::OnboardingRecordEnvironment for HostOnboardingEnvironment {
    fn load_user_id(&self) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn has_existing_local_task(&self) -> Result<bool, String> {
        self.repo
            .list_task_metas(&task_index_repo::ListTaskMetasParams::default())
            .map(|metas| !metas.is_empty())
            .map_err(|error| error.to_string())
    }
}
