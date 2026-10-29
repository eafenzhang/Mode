//! `commands` 服务的 6 个方法（同步实现，适配器丢进 `spawn_blocking`）。

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use atomic_file::{write_text_atomic, WriteOptions};
use serde_json::{Map, Value};

use crate::descriptor::{
    command_agent_sources, command_file_name, command_name, get_command_source_descriptor,
    CommandAgentSourceDescriptor, COMMAND_DIRECTORY_SOURCE_DESCRIPTORS,
};
use crate::error::{commands_error, CommandsResult};
use crate::parser::{
    generate_command_file_content, parse_command_file, writable_command_config, ParsedCommandFile,
};
use crate::plugin::{
    canonical_plugin_id, is_default_enabled_official_plugin_id, read_installed_plugin_roots,
    resolve_official_plugin_cache_root, InstalledPluginRoot, MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
};
use crate::types::*;

const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";
const ENABLE_OVERRIDE_KEY: &str = "enable";
const HOME_PREFIX: &str = "~/";
const MODE_INLINE_PLUGIN_MARKETPLACE: &str = "inline";
const MODE_PLUGIN_MANIFEST_PATH: &[&str] = &[".mode-plugin", "plugin.json"];
/// 改名前的清单目录（`.zcode-plugin`）：继续可读。
const MODE_LEGACY_MANIFEST_PATH: &[&str] = &[".zcode-plugin", "plugin.json"];
const CLAUDE_PLUGIN_MANIFEST_PATH: &[&str] = &[".claude-plugin", "plugin.json"];
const CODEX_PLUGIN_MANIFEST_PATH: &[&str] = &[".codex-plugin", "plugin.json"];

fn env_non_empty(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `os.homedir()` 的跨平台近似：`USERPROFILE` → `HOMEDRIVE`+`HOMEPATH` → `HOME`。
fn process_home() -> PathBuf {
    if let Some(profile) = env_non_empty("USERPROFILE") {
        return profile;
    }
    if let (Some(drive), Some(path)) = (std::env::var("HOMEDRIVE").ok(), std::env::var("HOMEPATH").ok()) {
        if !drive.is_empty() && !path.is_empty() {
            return PathBuf::from(format!("{drive}{path}"));
        }
    }
    env_non_empty("HOME").unwrap_or_else(|| PathBuf::from("."))
}

/// `resolveUserHomeDir`：`HOME`（trim 非空）→ `USERPROFILE`（trim 非空）→ 进程 home。
pub fn resolve_user_home_dir() -> PathBuf {
    env_non_empty("HOME")
        .or_else(|| env_non_empty("USERPROFILE"))
        .unwrap_or_else(process_home)
}

#[derive(Debug, Clone)]
pub struct CommandsServiceOptions {
    /// 用户 home：`{home}/.mode` 与 `{home}/.mode/cli/config.json` 的根。
    pub home: PathBuf,
    /// 相对路径型 `storage.dir` 的解析基准（Node 的 `resolve` 用进程 cwd）。
    pub cwd: PathBuf,
    /// 配置文件与命令文件的原子写参数（测试可关掉跨进程锁）。
    pub write_options: WriteOptions,
    /// 插件存储根的注入点（测试用）；`None` 时按 `storage.dir` 推导。
    pub plugin_storage_root: Option<PathBuf>,
}

impl CommandsServiceOptions {
    /// 默认从进程环境取：home 走 `HOME`/`USERPROFILE`，cwd 走进程当前目录。
    pub fn from_process() -> Self {
        let home = resolve_user_home_dir();
        let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
        Self {
            home,
            cwd,
            write_options: WriteOptions::default(),
            plugin_storage_root: None,
        }
    }

    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            cwd: home.clone(),
            home,
            write_options: WriteOptions::default(),
            plugin_storage_root: None,
        }
    }
}

pub struct CommandsService {
    options: CommandsServiceOptions,
}

struct StorageTarget {
    commands_root: PathBuf,
    scope: CommandScope,
    project_path: Option<String>,
}

struct DiscoveryOptions<'a> {
    descriptor: &'a CommandAgentSourceDescriptor,
    scope: CommandScope,
    enabled_overrides: &'a HashMap<String, bool>,
    project_path: Option<String>,
}

struct PluginManifestSummary {
    /// `None` 表示清单里没有 `commands` 字段（不是空数组）。
    commands: Option<Value>,
    name: String,
}

struct PluginCommandRootDescriptor {
    plugin_enabled: bool,
    plugin_marketplace: String,
    plugin_name: String,
    root_path: PathBuf,
}

struct PluginRootCandidate {
    default_enabled: bool,
    marketplace: String,
    root_path: PathBuf,
}

struct PluginConfigSummary {
    dirs: Vec<String>,
    enabled: bool,
    enabled_plugins: HashMap<String, bool>,
    storage_dir: String,
    suppressed_builtins: Vec<String>,
}

impl CommandsService {
    pub fn new(options: CommandsServiceOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &CommandsServiceOptions {
        &self.options
    }

    // ========================================================================
    // list
    // ========================================================================

    pub fn list(&self, params: &ListParams) -> CommandsResult<CommandsListResult> {
        let mut user_commands: Vec<UserCommand> = Vec::new();
        let enabled_overrides = self.read_command_enabled_overrides();
        let agent_sources = match params.agent_source {
            Some(agent_source) => vec![agent_source],
            None => command_agent_sources().to_vec(),
        };

        // Mode Agent 先把所有 workspace 目录合并，再合并所有 user 目录；按目录交错读
        // project/user 会让 user `.mode` 抢在 workspace `.agents` 前面。
        for agent_source in agent_sources {
            let descriptors: Vec<&CommandAgentSourceDescriptor> = if agent_source
                == CommandAgentSource::ModeAgent
            {
                COMMAND_DIRECTORY_SOURCE_DESCRIPTORS.iter().collect()
            } else {
                vec![get_command_source_descriptor(agent_source)]
            };

            if let Some(workspace_path) = params.workspace_path.as_deref() {
                self.discover_commands_from_directory_sources(
                    &descriptors,
                    |descriptor| {
                        descriptor
                            .workspace_directory_segments
                            .iter()
                            .fold(PathBuf::from(workspace_path), |path, segment| {
                                path.join(segment)
                            })
                    },
                    CommandScope::Project,
                    Some(workspace_path.to_string()),
                    &mut user_commands,
                    &enabled_overrides,
                );
            }

            self.discover_commands_from_directory_sources(
                &descriptors,
                |descriptor| self.user_commands_root_for_descriptor(descriptor),
                CommandScope::Global,
                None,
                &mut user_commands,
                &enabled_overrides,
            );
        }

        let deduped_user_commands = dedupe_commands_by_name(user_commands);
        let plugin_commands = match params.agent_source {
            None | Some(CommandAgentSource::ModeAgent) => {
                self.discover_plugin_commands(&enabled_overrides)
            }
        };

        let mut commands: Vec<ModeCommand> = deduped_user_commands
            .iter()
            .cloned()
            .map(ModeCommand::User)
            .collect();
        commands.extend(plugin_commands.iter().cloned().map(ModeCommand::Plugin));

        Ok(CommandsListResult {
            commands,
            user_commands: deduped_user_commands,
            plugin_commands,
            capability: CommandsCapability {
                user_scope_available: true,
            },
        })
    }

    // ========================================================================
    // writeCommandFile
    // ========================================================================

    pub fn write_command_file(&self, params: &CommandCreateParams) -> CommandsResult<WriteCommandResult> {
        let agent_source = params.agent_source.unwrap_or_default();
        let descriptor = get_command_source_descriptor(agent_source);
        let target = self.commands_root_for_storage(
            descriptor,
            params.storage_level,
            params.workspace_path.as_deref(),
        )?;
        std::fs::create_dir_all(&target.commands_root)?;

        let file_name = command_file_name(&params.config, descriptor);
        let file_path = self.resolve_command_write_target(
            &target.commands_root,
            &file_name,
            &params.config.name,
        )?;

        // TS 用 `access()`：只有 ENOENT 才算「不存在」，其它访问错误原样抛出。
        ensure_command_file_absent(&file_path, &file_name)?;

        let content = generate_command_file_content(
            &writable_command_config(&params.config, descriptor.supports_argument_hint),
            None,
        );

        // TS 只 mkdir 命令根，不替用户创建命名空间子目录：父目录缺失就是 ENOENT。
        let parent_exists = file_path
            .parent()
            .map(Path::exists)
            .unwrap_or(false);
        if !parent_exists {
            return Err(commands_error(format!(
                "ENOENT: no such file or directory, open '{}'",
                file_path.to_string_lossy()
            )));
        }

        write_text_atomic(&file_path, &content, &self.options.write_options)
            .map_err(|error| commands_error(error.to_string()))?;

        let mut config = self.read_user_cli_config();
        set_command_enabled_override(&mut config, &path_key(&file_path), true);
        self.write_user_cli_config(&config)?;

        let parsed = parse_command_file(&content, &path_key(&file_path));
        let name = command_name(
            &target.commands_root,
            &file_path,
            descriptor.namespace_separator,
        );
        let location = build_command_location(
            descriptor,
            &target.commands_root,
            target.scope,
            target.project_path.as_deref(),
        );

        let command = user_command_from_parsed(
            parsed,
            name,
            agent_source,
            location,
            true,
            target.scope,
            target.project_path.clone(),
        );

        Ok(WriteCommandResult { command })
    }

    // ========================================================================
    // updateCommandFile
    // ========================================================================

    pub fn update_command_file(&self, params: &CommandUpdateParams) -> CommandsResult<WriteCommandResult> {
        let agent_source = params.agent_source.unwrap_or_default();
        let descriptor = get_command_source_descriptor(agent_source);
        let target = self.commands_root_for_storage(
            descriptor,
            params.storage_level,
            params.workspace_path.as_deref(),
        )?;
        let new_file_name = command_file_name(&params.config, descriptor);
        let new_file_path = self.resolve_command_write_target(
            &target.commands_root,
            &new_file_name,
            &params.config.name,
        )?;
        let new_file_key = path_key(&new_file_path);

        let enabled_overrides = self.read_command_enabled_overrides();
        let existing_content = params
            .old_file_path
            .as_deref()
            .and_then(|old_path| std::fs::read(old_path).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());

        let same_path = params.old_file_path.as_deref() == Some(new_file_key.as_str());

        // 文件名变了，先删旧文件（旧文件可能已被移除，失败忽略）。
        if let Some(old_file_path) = params.old_file_path.as_deref() {
            if !same_path {
                let _ = std::fs::remove_file(old_file_path);
            }
        }

        // 新文件已存在（排除自己的旧路径）；同样只把 ENOENT 当作不存在。
        if !same_path {
            ensure_command_file_absent(&new_file_path, &new_file_name)?;
        }

        let content = generate_command_file_content(
            &writable_command_config(&params.config, descriptor.supports_argument_hint),
            existing_content.as_deref(),
        );
        if let Some(parent) = new_file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_text_atomic(&new_file_path, &content, &self.options.write_options)
            .map_err(|error| commands_error(error.to_string()))?;

        if let Some(old_file_path) = params.old_file_path.as_deref() {
            if !same_path {
                if let Some(previous) = enabled_overrides.get(old_file_path).copied() {
                    // 禁用状态按命令文件路径存放；改名会换路径，必须迁移 override，
                    // 否则用户刚禁用的命令会因为改名重新启用。
                    let mut config = self.read_user_cli_config();
                    set_command_enabled_override(&mut config, old_file_path, true);
                    set_command_enabled_override(&mut config, &new_file_key, previous);
                    self.write_user_cli_config(&config)?;
                }
            }
        }

        let parsed = parse_command_file(&content, &new_file_key);
        let name = command_name(
            &target.commands_root,
            &new_file_path,
            descriptor.namespace_separator,
        );
        let location = build_command_location(
            descriptor,
            &target.commands_root,
            target.scope,
            target.project_path.as_deref(),
        );
        let enabled = self
            .read_command_enabled_overrides()
            .get(&new_file_key)
            .copied()
            .unwrap_or(true);

        let command = user_command_from_parsed(
            parsed,
            name,
            agent_source,
            location,
            enabled,
            target.scope,
            target.project_path.clone(),
        );

        Ok(WriteCommandResult { command })
    }

    // ========================================================================
    // deleteCommandFile / setCommandEnabled / getPrimaryUserCommandsDirectory
    // ========================================================================

    pub fn delete_command_file(&self, params: &CommandDeleteParams) -> CommandsResult<()> {
        match std::fs::remove_file(&params.file_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(commands_error(error.to_string())),
        }

        let mut config = self.read_user_cli_config();
        set_command_enabled_override(&mut config, &params.file_path, true);
        self.write_user_cli_config(&config)
    }

    pub fn set_command_enabled(&self, params: &CommandSetEnabledParams) -> CommandsResult<()> {
        let mut config = self.read_user_cli_config();
        set_command_enabled_override(&mut config, &params.file_path, params.enabled);
        self.write_user_cli_config(&config)
    }

    pub fn get_primary_user_commands_directory(
        &self,
        params: &PrimaryDirectoryParams,
    ) -> CommandsResult<PrimaryDirectoryResult> {
        let descriptor = get_command_source_descriptor(params.agent_source.unwrap_or_default());
        let path = self.user_commands_root_for_descriptor(descriptor);
        // open-in-file-manager 在 Windows 上不能可靠打开不存在的路径。
        std::fs::create_dir_all(&path)?;
        Ok(PrimaryDirectoryResult {
            path: path_key(&path),
        })
    }

    // ========================================================================
    // 目录与路径
    // ========================================================================

    fn user_commands_root_for_descriptor(&self, descriptor: &CommandAgentSourceDescriptor) -> PathBuf {
        descriptor
            .user_directory_segments
            .iter()
            .fold(self.options.home.clone(), |path, segment| path.join(segment))
    }

    fn user_cli_config_path(&self) -> PathBuf {
        self.options
            .home
            .join(MODE_DATA_ROOT_DIR_NAME)
            .join("cli")
            .join("config.json")
    }

    fn commands_root_for_storage(
        &self,
        descriptor: &CommandAgentSourceDescriptor,
        storage_level: Option<CommandStorageLevel>,
        workspace_path: Option<&str>,
    ) -> CommandsResult<StorageTarget> {
        if storage_level == Some(CommandStorageLevel::Project) {
            let Some(workspace_path) = workspace_path else {
                return Err(commands_error("Missing workspace path for project command"));
            };
            let commands_root = descriptor
                .workspace_directory_segments
                .iter()
                .fold(PathBuf::from(workspace_path), |path, segment| path.join(segment));
            return Ok(StorageTarget {
                commands_root,
                scope: CommandScope::Project,
                project_path: Some(workspace_path.to_string()),
            });
        }

        Ok(StorageTarget {
            commands_root: self.user_commands_root_for_descriptor(descriptor),
            scope: CommandScope::Global,
            project_path: None,
        })
    }

    /// 目标路径必须落在命令根内：绝对路径/`..` 逃逸一律拒绝（TS 会直接 join 写出根外）。
    fn resolve_command_write_target(
        &self,
        commands_root: &Path,
        file_name: &str,
        raw_name: &str,
    ) -> CommandsResult<PathBuf> {
        let invalid = || commands_error(format!("Invalid command file name: {raw_name}"));
        let relative = Path::new(file_name);
        if relative.has_root() {
            return Err(invalid());
        }
        let root = normalize_path(commands_root);
        let resolved = normalize_path(&root.join(relative));
        let Ok(inside) = resolved.strip_prefix(&root) else {
            return Err(invalid());
        };
        if inside.as_os_str().is_empty()
            || inside
                .components()
                .any(|component| component == Component::ParentDir)
            || inside.to_string_lossy().starts_with("..")
        {
            return Err(invalid());
        }
        Ok(resolved)
    }

    fn resolve_config_path(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix(HOME_PREFIX) {
            return normalize_path(&self.options.home.join(rest));
        }
        let candidate = PathBuf::from(path);
        if candidate.has_root() {
            normalize_path(&candidate)
        } else {
            normalize_path(&self.options.cwd.join(candidate))
        }
    }

    fn plugin_storage_root_for(&self, storage_dir: &str) -> PathBuf {
        if let Some(root) = &self.options.plugin_storage_root {
            return root.clone();
        }
        resolve_plugin_storage_root(&self.resolve_config_path(storage_dir))
    }

    // ========================================================================
    // 用户命令发现
    // ========================================================================

    fn discover_commands_from_directory_sources(
        &self,
        descriptors: &[&CommandAgentSourceDescriptor],
        commands_root_for_descriptor: impl Fn(&CommandAgentSourceDescriptor) -> PathBuf,
        scope: CommandScope,
        project_path: Option<String>,
        commands: &mut Vec<UserCommand>,
        enabled_overrides: &HashMap<String, bool>,
    ) {
        for descriptor in descriptors {
            let discovered_count = self.discover_commands_root(
                descriptor,
                &commands_root_for_descriptor(descriptor),
                scope,
                project_path.clone(),
                commands,
                enabled_overrides,
            );
            // `.mode` 是强优先级来源；只要读到有效命令，同 scope 的 `.agents` 就不再参与。
            if descriptor.directory_source == SettingsDirectorySource::Mode && discovered_count > 0 {
                break;
            }
        }
    }

    fn discover_commands_root(
        &self,
        descriptor: &CommandAgentSourceDescriptor,
        commands_root: &Path,
        scope: CommandScope,
        project_path: Option<String>,
        commands: &mut Vec<UserCommand>,
        enabled_overrides: &HashMap<String, bool>,
    ) -> usize {
        let before_count = commands.len();
        if !commands_root.exists() {
            return 0;
        }

        let options = DiscoveryOptions {
            descriptor,
            scope,
            enabled_overrides,
            project_path,
        };
        self.discover_user_commands_recursive(commands_root, commands_root, commands, &options);
        commands.len() - before_count
    }

    fn discover_user_commands_recursive(
        &self,
        root_dir: &Path,
        current_dir: &Path,
        commands: &mut Vec<UserCommand>,
        options: &DiscoveryOptions<'_>,
    ) {
        let Ok(entries) = std::fs::read_dir(current_dir) else {
            return;
        };

        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            // 跳过隐藏文件和目录。
            if entry_name.starts_with('.') {
                continue;
            }

            let full_path = entry.path();
            // lstat 语义：符号链接目录不跟随（与 TS `lstat().isDirectory()` 一致）。
            let Ok(metadata) = std::fs::symlink_metadata(&full_path) else {
                continue;
            };

            if metadata.is_dir() {
                self.discover_user_commands_recursive(root_dir, &full_path, commands, options);
                continue;
            }

            if !entry_name
                .to_lowercase()
                .ends_with(options.descriptor.file_extension)
            {
                continue;
            }

            let Ok(bytes) = std::fs::read(&full_path) else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes);
            let file_path = path_key(&full_path);
            let parsed = parse_command_file(&content, &file_path);
            let name = command_name(root_dir, &full_path, options.descriptor.namespace_separator);
            let location = build_command_location(
                options.descriptor,
                root_dir,
                options.scope,
                options.project_path.as_deref(),
            );
            let enabled = options
                .enabled_overrides
                .get(&file_path)
                .copied()
                .unwrap_or(true);

            commands.push(user_command_from_parsed(
                parsed,
                name,
                options.descriptor.agent_source,
                location,
                enabled,
                options.scope,
                options.project_path.clone(),
            ));
        }
    }

    // ========================================================================
    // 插件命令
    // ========================================================================

    fn discover_plugin_commands(
        &self,
        enabled_overrides: &HashMap<String, bool>,
    ) -> Vec<PluginCommand> {
        let root_descriptors = self.resolve_plugin_command_root_descriptors();
        let mut commands: Vec<PluginCommand> = Vec::new();
        let mut seen_file_paths: HashSet<String> = HashSet::new();
        for descriptor in &root_descriptors {
            self.discover_plugin_commands_recursive(
                &descriptor.root_path,
                &descriptor.root_path,
                &mut commands,
                descriptor,
                enabled_overrides,
                &mut seen_file_paths,
            );
        }
        commands.sort_by(|left, right| compare_names(&left.name, &right.name));
        commands
    }

    fn discover_plugin_commands_recursive(
        &self,
        root_dir: &Path,
        current_dir: &Path,
        commands: &mut Vec<PluginCommand>,
        descriptor: &PluginCommandRootDescriptor,
        enabled_overrides: &HashMap<String, bool>,
        seen_file_paths: &mut HashSet<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(current_dir) else {
            return;
        };

        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            if entry_name.starts_with('.') {
                continue;
            }

            let full_path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&full_path) else {
                continue;
            };

            if metadata.is_dir() {
                self.discover_plugin_commands_recursive(
                    root_dir,
                    &full_path,
                    commands,
                    descriptor,
                    enabled_overrides,
                    seen_file_paths,
                );
                continue;
            }

            if !entry_name
                .to_lowercase()
                .ends_with(crate::descriptor::MODE_COMMAND_DESCRIPTOR.file_extension)
            {
                continue;
            }

            let Ok(bytes) = std::fs::read(&full_path) else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes);
            let file_path = path_key(&full_path);
            let parsed = parse_command_file(&content, &file_path);
            let name = command_name(
                root_dir,
                &full_path,
                crate::descriptor::MODE_COMMAND_DESCRIPTOR.namespace_separator,
            );

            let command_key = file_path.replace('\\', "/").to_lowercase();
            if !seen_file_paths.insert(command_key) {
                continue;
            }

            let enabled = descriptor.plugin_enabled
                && enabled_overrides.get(&file_path).copied().unwrap_or(true);

            commands.push(PluginCommand {
                name: name.clone(),
                prompt: parsed.prompt,
                content: parsed.content,
                description: parsed.description,
                argument_hint: parsed.argument_hint,
                id: build_plugin_command_id(
                    &file_path,
                    &name,
                    &descriptor.plugin_marketplace,
                    &descriptor.plugin_name,
                ),
                source: CommandSource::Plugin,
                enabled,
                plugin_name: descriptor.plugin_name.clone(),
                plugin_marketplace: descriptor.plugin_marketplace.clone(),
                plugin_enabled: descriptor.plugin_enabled,
                scope: CommandScope::Global,
                file_path,
            });
        }
    }

    fn resolve_plugin_command_root_descriptors(&self) -> Vec<PluginCommandRootDescriptor> {
        let config = self.read_user_cli_config();
        let plugin_config = read_plugin_config_from_config(&config);
        if !plugin_config.enabled {
            return Vec::new();
        }

        let plugin_storage_root = self.plugin_storage_root_for(&plugin_config.storage_dir);
        let official_cache_roots = scan_official_plugin_cache_roots(&plugin_storage_root);
        let installed_roots = read_installed_plugin_roots(&plugin_storage_root);

        let mut candidates: Vec<PluginRootCandidate> = Vec::new();
        for dir in &plugin_config.dirs {
            candidates.push(PluginRootCandidate {
                default_enabled: true,
                marketplace: MODE_INLINE_PLUGIN_MARKETPLACE.to_string(),
                root_path: self.resolve_config_path(dir),
            });
        }
        for root_path in official_cache_roots {
            candidates.push(PluginRootCandidate {
                default_enabled: false,
                marketplace: MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID.to_string(),
                root_path,
            });
        }
        for InstalledPluginRoot {
            default_enabled,
            marketplace,
            root_path,
        } in installed_roots
        {
            candidates.push(PluginRootCandidate {
                default_enabled,
                marketplace,
                root_path,
            });
        }

        let mut descriptors: Vec<PluginCommandRootDescriptor> = Vec::new();
        let mut seen_plugin_ids: HashSet<String> = HashSet::new();

        for candidate in candidates {
            let Some(manifest) = read_plugin_manifest(&candidate.root_path) else {
                continue;
            };
            let plugin_id = format!("{}@{}", manifest.name, candidate.marketplace);
            // 内置官方插件被「卸载」后只在 CLI config 写入 suppressedBuiltins。
            if candidate.marketplace == MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID
                && plugin_config.suppressed_builtins.contains(&plugin_id)
            {
                continue;
            }
            if !seen_plugin_ids.insert(plugin_id.clone()) {
                continue;
            }
            let default_enabled =
                candidate.default_enabled || is_default_enabled_official_plugin_id(&plugin_id);
            let enabled = plugin_config
                .enabled_plugins
                .get(&plugin_id)
                .copied()
                .unwrap_or(default_enabled);
            if !enabled {
                continue;
            }

            let command_roots = resolve_plugin_command_roots(&manifest, &candidate.root_path);
            for root_path in command_roots {
                descriptors.push(PluginCommandRootDescriptor {
                    plugin_enabled: enabled,
                    plugin_marketplace: candidate.marketplace.clone(),
                    plugin_name: manifest.name.clone(),
                    root_path,
                });
            }
        }

        descriptors
    }

    // ========================================================================
    // `~/.mode/cli/config.json`
    // ========================================================================

    fn read_user_cli_config(&self) -> Map<String, Value> {
        let Ok(raw) = std::fs::read(self.user_cli_config_path()) else {
            return Map::new();
        };
        let Ok(parsed) = serde_json::from_slice::<Value>(&raw) else {
            return Map::new();
        };
        match parsed {
            Value::Object(object) => object,
            _ => Map::new(),
        }
    }

    fn write_user_cli_config(&self, config: &Map<String, Value>) -> CommandsResult<()> {
        let file_path = self.user_cli_config_path();
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(config.clone()))
            .map_err(|error| commands_error(error.to_string()))?;
        text.push('\n');
        write_text_atomic(&file_path, &text, &self.options.write_options)
            .map_err(|error| commands_error(error.to_string()))
    }

    fn read_command_enabled_overrides(&self) -> HashMap<String, bool> {
        read_command_enabled_overrides(&self.read_user_cli_config())
    }
}

// ============================================================================
// 模块级辅助
// ============================================================================

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 对齐 TS 的 `access()` + `ENOENT` 判定：存在即报「已存在」，其它访问错误原样上抛。
fn ensure_command_file_absent(path: &Path, file_name: &str) -> CommandsResult<()> {
    match std::fs::metadata(path) {
        Ok(_) => Err(commands_error(format!(
            "Command file already exists: {file_name}"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(commands_error(error.to_string())),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn user_command_from_parsed(
    parsed: ParsedCommandFile,
    name: String,
    agent_source: CommandAgentSource,
    location: SettingsDirectoryLocation,
    enabled: bool,
    scope: CommandScope,
    project_path: Option<String>,
) -> UserCommand {
    let id = format!(
        "{}:{}:{}:{}",
        agent_source_key(agent_source),
        directory_source_key(location.source),
        scope_key(scope),
        name
    );
    UserCommand {
        name,
        prompt: parsed.prompt,
        content: parsed.content,
        file_path: parsed.file_path,
        description: parsed.description,
        argument_hint: parsed.argument_hint,
        id,
        source: CommandSource::User,
        agent_source,
        location,
        enabled,
        scope,
        project_path,
    }
}

fn agent_source_key(agent_source: CommandAgentSource) -> &'static str {
    match agent_source {
        CommandAgentSource::ModeAgent => "modeAgent",
    }
}

fn directory_source_key(source: SettingsDirectorySource) -> &'static str {
    match source {
        SettingsDirectorySource::Mode => "mode",
        SettingsDirectorySource::Agents => "agents",
        SettingsDirectorySource::Claude => "claude",
    }
}

fn scope_key(scope: CommandScope) -> &'static str {
    match scope {
        CommandScope::Global => "global",
        CommandScope::Project => "project",
    }
}

fn build_command_location(
    descriptor: &CommandAgentSourceDescriptor,
    commands_root: &Path,
    scope: CommandScope,
    project_path: Option<&str>,
) -> SettingsDirectoryLocation {
    SettingsDirectoryLocation {
        source: descriptor.directory_source,
        scope: match scope {
            CommandScope::Project => SettingsDirectoryScope::Project,
            CommandScope::Global => SettingsDirectoryScope::User,
        },
        directory_path: path_key(commands_root),
        project_path: project_path.map(str::to_string),
    }
}

fn read_command_enabled_overrides(config: &Map<String, Value>) -> HashMap<String, bool> {
    let mut overrides = HashMap::new();
    let Some(command_config) = config.get("command").and_then(|value| value.as_object()) else {
        return overrides;
    };
    for (file_path, entry) in command_config {
        if let Some(enabled) = entry
            .as_object()
            .and_then(|entry| entry.get(ENABLE_OVERRIDE_KEY))
            .and_then(|value| value.as_bool())
        {
            overrides.insert(file_path.clone(), enabled);
        }
    }
    overrides
}

fn set_command_enabled_override(
    config: &mut Map<String, Value>,
    file_path: &str,
    enabled: bool,
) {
    let mut command_config = match config.get("command") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };

    if enabled {
        command_config.remove(file_path);
    } else {
        let mut entry = Map::new();
        entry.insert(ENABLE_OVERRIDE_KEY.to_string(), Value::Bool(false));
        command_config.insert(file_path.to_string(), Value::Object(entry));
    }

    if command_config.is_empty() {
        config.remove("command");
    } else {
        config.insert("command".to_string(), Value::Object(command_config));
    }
}

fn dedupe_commands_by_name(commands: Vec<UserCommand>) -> Vec<UserCommand> {
    let mut selected: HashMap<String, UserCommand> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for command in commands {
        if !selected.contains_key(&command.name) {
            order.push(command.name.clone());
            selected.insert(command.name.clone(), command);
        }
    }
    order
        .into_iter()
        .filter_map(|name| selected.remove(&name))
        .collect()
}

fn build_plugin_command_id(
    file_path: &str,
    name: &str,
    plugin_marketplace: &str,
    plugin_name: &str,
) -> String {
    format!("plugin:{plugin_marketplace}:{plugin_name}:{name}:{file_path}")
}

/// JS `localeCompare` 依赖运行环境 locale；这里用确定性口径（忽略大小写 → 原文 → 码点）。
fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn read_string_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn read_boolean_record(value: Option<&Value>) -> HashMap<String, bool> {
    let mut result = HashMap::new();
    let Some(object) = value.and_then(|value| value.as_object()) else {
        return result;
    };
    for (key, enabled) in object {
        if let Some(enabled) = enabled.as_bool() {
            result.insert(key.clone(), enabled);
        }
    }
    result
}

fn read_storage_dir_from_config(config: &Map<String, Value>) -> String {
    let storage = config.get("storage").and_then(|value| value.as_object());
    let dir = storage
        .and_then(|storage| storage.get("dir"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("");
    if dir.is_empty() {
        format!("{HOME_PREFIX}{MODE_DATA_ROOT_DIR_NAME}")
    } else {
        dir.to_string()
    }
}

fn read_plugin_config_from_config(config: &Map<String, Value>) -> PluginConfigSummary {
    let plugins = config.get("plugins").and_then(|value| value.as_object());
    let mut enabled_plugins = HashMap::new();
    for (id, enabled) in read_boolean_record(plugins.and_then(|plugins| plugins.get("enabledPlugins"))) {
        enabled_plugins.insert(canonical_plugin_id(&id), enabled);
    }
    PluginConfigSummary {
        dirs: read_string_array(plugins.and_then(|plugins| plugins.get("dirs"))),
        enabled: plugins
            .and_then(|plugins| plugins.get("enabled"))
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
        enabled_plugins,
        storage_dir: read_storage_dir_from_config(config),
        suppressed_builtins: read_string_array(
            plugins.and_then(|plugins| plugins.get("suppressedBuiltins")),
        )
        .into_iter()
        .map(|id| canonical_plugin_id(&id))
        .collect(),
    }
}

fn resolve_cli_storage_root(storage_root: &Path) -> PathBuf {
    let is_cli = storage_root
        .file_name()
        .map(|name| name == "cli")
        .unwrap_or(false);
    if is_cli {
        storage_root.to_path_buf()
    } else {
        storage_root.join("cli")
    }
}

fn resolve_plugin_storage_root(storage_root: &Path) -> PathBuf {
    resolve_cli_storage_root(storage_root).join("plugins")
}

fn scan_official_plugin_cache_roots(plugin_storage_root: &Path) -> Vec<PathBuf> {
    let cache_root = resolve_official_plugin_cache_root(plugin_storage_root);
    let Ok(plugin_entries) = std::fs::read_dir(&cache_root) else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for plugin_entry in plugin_entries.flatten() {
        let plugin_dir = plugin_entry.path();
        let Ok(version_entries) = std::fs::read_dir(&plugin_dir) else {
            continue;
        };
        for version_entry in version_entries.flatten() {
            let root_path = version_entry.path();
            if std::fs::symlink_metadata(&root_path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
            {
                roots.push(root_path);
            }
        }
    }
    roots.sort();
    roots
}

fn find_plugin_manifest_path(root_path: &Path) -> Option<PathBuf> {
    [
        MODE_PLUGIN_MANIFEST_PATH,
        MODE_LEGACY_MANIFEST_PATH,
        CLAUDE_PLUGIN_MANIFEST_PATH,
        CODEX_PLUGIN_MANIFEST_PATH,
    ]
    .iter()
    .map(|segments| segments.iter().fold(root_path.to_path_buf(), |path, segment| path.join(segment)))
    .find(|path| path.exists())
}

fn read_plugin_manifest(root_path: &Path) -> Option<PluginManifestSummary> {
    let manifest_path = find_plugin_manifest_path(root_path)?;
    let raw = std::fs::read(manifest_path).ok()?;
    let parsed = serde_json::from_slice::<Value>(&raw).ok()?;
    let object = parsed.as_object()?;
    let name = object
        .get("name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("");
    if !is_valid_plugin_name(name) {
        return None;
    }
    Some(PluginManifestSummary {
        commands: object.get("commands").cloned(),
        name: name.to_string(),
    })
}

/// `^[a-z0-9][a-z0-9._-]{0,127}$`
fn is_valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes[1..].iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    })
}

fn parse_path_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(single)) => vec![single.clone()],
        Some(other) => read_string_array(Some(other)),
        None => Vec::new(),
    }
}

fn resolve_inside(root_path: &Path, raw_path: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(raw_path);
    if candidate.has_root() {
        return None;
    }
    let root = normalize_path(root_path);
    let resolved = normalize_path(&root.join(candidate));
    let relative = resolved.strip_prefix(&root).ok()?;
    if relative.as_os_str().is_empty() {
        return Some(resolved);
    }
    if relative.to_string_lossy().starts_with("..") {
        return None;
    }
    Some(resolved)
}

fn resolve_plugin_command_roots(
    manifest: &PluginManifestSummary,
    root_path: &Path,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for raw_path in parse_path_list(manifest.commands.as_ref()) {
        if let Some(resolved) = resolve_inside(root_path, &raw_path) {
            roots.push(resolved);
        }
    }
    if roots.is_empty() && manifest.commands.is_none() {
        let default_root = root_path.join("commands");
        if default_root.exists() {
            roots.push(default_root);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_parent_segments() {
        assert_eq!(
            normalize_path(Path::new("C:/a/b/../c/./d")),
            PathBuf::from("C:/a/c/d")
        );
    }

    #[test]
    fn resolve_inside_rejects_escapes_and_absolute_paths() {
        let root = Path::new("C:/plugins/one");
        assert_eq!(
            resolve_inside(root, "commands"),
            Some(PathBuf::from("C:/plugins/one/commands"))
        );
        assert_eq!(resolve_inside(root, "../two/commands"), None);
        assert_eq!(resolve_inside(root, "C:/other"), None);
        assert_eq!(resolve_inside(root, "..hidden"), None);
    }

    #[test]
    fn override_round_trip() {
        let mut config = Map::new();
        config.insert("theme".into(), Value::String("dark".into()));
        set_command_enabled_override(&mut config, "C:/a.md", false);
        assert_eq!(
            config.get("command").unwrap()["C:/a.md"]["enable"],
            Value::Bool(false)
        );
        set_command_enabled_override(&mut config, "C:/a.md", true);
        assert!(!config.contains_key("command"));
        assert_eq!(config.get("theme").unwrap(), &Value::String("dark".into()));
    }

    #[test]
    fn plugin_name_validation_matches_ts_regex() {
        assert!(is_valid_plugin_name("abc"));
        assert!(is_valid_plugin_name("a-b_c.d"));
        assert!(!is_valid_plugin_name("Abc"));
        assert!(!is_valid_plugin_name("-abc"));
        assert!(!is_valid_plugin_name(&"a".repeat(129)));
        assert!(is_valid_plugin_name(&"a".repeat(128)));
    }

    #[test]
    fn overrides_ignore_non_boolean_entries_and_default_to_true() {
        let mut config = Map::new();
        assert!(read_command_enabled_overrides(&config).is_empty());
        config.insert(
            "command".into(),
            serde_json::json!({ "x": { "enable": false }, "y": { "enable": "no" }, "z": 1 }),
        );
        let overrides = read_command_enabled_overrides(&config);
        assert_eq!(overrides.get("x"), Some(&false));
        assert!(!overrides.contains_key("y"));
        assert!(!overrides.contains_key("z"));
    }
}
