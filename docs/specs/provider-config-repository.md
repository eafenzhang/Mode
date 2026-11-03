# Spec：Personal Provider Config 仓库（Rust）

> 范围：`packages/provider-node/src/personal-provider-config-repository.ts` 的
> **读/写/锁/原子写/legacy 导入**，以及 `runtime-paths.ts` 的环境变量路径解析。
> 只读文件层与两层组合快照见 `provider-config-source.md`；纯编解码见
> `provider-config-file-codec.md`。本文件包含读/写与轮询/`onDidChange`。

## 1. 范围与所有者

| Rust 入口 | TS 对应 |
| --- | --- |
| `provider-config-repository::PersonalConfigRepository` | `NodePersonalProviderConfigRepository` |
| `provider-config-repository::runtime_paths` | `runtime-paths.ts` |
| `atomic-file` | `withFileLock` / `atomicWritePrivateTextFile` |
| `provider-config-file-codec` | `decode/encodeProviderConfigFile` |

- **唯一所有者**：仓库持有 Personal 配置文件路径；所有读改写都经它，频道/服务不另建
  写路径。
- **读**：文件缺失且无 legacy 导入时返回空层；存在时严格解码，非规范内容在锁内重读、
  规范化并写回；无效文件不备份、不覆盖，进程内降级为空层并上报恢复事件。
- **写**：`update(transform)` 在跨进程锁内读当前快照、应用变换、严格校验整份结果、
  原子写回（Unix 0600），返回规范化后的快照。
- **legacy 导入**：文件缺失时调用注入的 `import_legacy`；返回 `Some(update)` 则写盘，
  `None` 则返回空层。导入也在锁内重读后执行。

## 2. API

```rust
pub enum PollingInterval {
    Disabled,      // 对应 TS pollingIntervalMs: false
    Millis(u64),   // 缺省 1000
}

pub struct PersonalConfigRepositoryOptions {
    pub file_path: PathBuf,
    pub import_legacy: Option<Arc<dyn Fn() -> Option<ProviderConfigLayerUpdate> + Send + Sync>>,
    pub on_recovery: Option<Arc<dyn Fn(&RepositoryError) + Send + Sync>>,
    pub on_polling_error: Option<Arc<dyn Fn(&RepositoryError) + Send + Sync>>,
    pub polling_interval: PollingInterval,
}

impl PersonalConfigRepository {
    pub fn new(options: PersonalConfigRepositoryOptions) -> Result<Self, RepositoryError>;
    pub fn read(&self) -> PersonalLayer;
    pub fn update<F>(&self, transform: F) -> Result<PersonalLayer, RepositoryError>
    where F: FnOnce(&PersonalLayer) -> ProviderConfigLayerUpdate;
    pub fn on_did_change(&self, listener: Arc<dyn Fn(&str) + Send + Sync>) -> ChangeSubscription;
    pub fn dispose(&self);
}
```

- `read` 不返回错误：任何读取/解码失败都降级为空层并调用 `on_recovery`（TS 语义）。
- `update` 返回错误；错误类型区分 IO / 非法内容 / 锁超时。
- `file_path` 为空串在构造时拒绝：`Personal Provider Config filePath 不能为空`；
  `polling_interval = Millis(0)` 拒绝：`Personal Provider Config pollingIntervalMs 必须大于 0`。
- 快照 `PersonalLayer` 来自 `provider-config-source`，revision 为规范编码文本的
  sha256（与 TS `snapshotFromUpdate` 一致）。
- 轮询在首次 `read`/`update` 后启动（与 TS `#ensurePolling` 一致），默认 1000 ms；
  `Disabled` 不启动。`on_did_change` 订阅变更原因：`updated`、`poll-changed`、`poll-error`。
  `dispose` 幂等：停止线程、清空订阅者；之后 `update` 返回已 dispose 错误。

## 3. 读路径

```text
read()
  ├─ 读文件（不存在 → 无 legacy 时空层）
  ├─ 严格解码 + 规范编码比较
  │    └─ 已规范 → 返回快照
  └─ 不一致（含 legacy 导入/需要规范化）
       └─ 跨进程锁 → 重新读文件 → 导入/规范化 → 原子写 → 返回快照
```

- 纯读不抢锁：正式文件由同目录临时文件原子替换，读到的要么是旧完整文档，要么是新完整
  文档（与 TS 注释一致）。
- 规范化写回使用 `use_file_lock = false`，因为外层已持锁；否则会自锁超时。
- 无效文件（JSON 损坏、schemaVersion 不支持、规则非法）**原样保留**，只在进程内降级。

## 4. 写路径与失败语义

| 场景 | 行为 |
| --- | --- |
| `file_path` 为空 | 构造失败 |
| 文件缺失 + 无 legacy | 空层，不建文件 |
| 文件缺失 + legacy 命中 | 锁内写回导入结果 |
| 文件缺失 + legacy 返回 None | 空层，不建文件 |
| 文件存在但非规范 | 锁内重读后规范化写回 |
| 文件无效 | `read` 降级空层并上报；`update` 返回错误，文件不动 |
| 锁超时 | `update` 返回锁错误（含等待毫秒数），不写文件 |
| 原子写失败 | 返回 IO 错误；临时文件清理由 `atomic-file` 负责 |

## 5. 运行时路径

```rust
pub const MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV: &str = "MODE_BUILTIN_PROVIDER_CONFIG_FILE";
pub const MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV: &str =
    "MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE";
pub const MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV: &str = "MODE_PERSONAL_PROVIDER_CONFIG_FILE";
pub const PERSONAL_PROVIDER_CONFIG_FILE_NAME: &str = "provider_config.json";

pub fn resolve_node_provider_runtime_paths(
    env: &BTreeMap<String, String>,
) -> Result<Option<ProviderRuntimePaths>, RuntimePathError>;
```

- 两个路径都为空 → `None`；只提供一个 → 错误
  `Mode Built-in 与 Personal Provider Config 路径必须同时提供`；值先 trim。
- `create_node_provider_runtime_path_env(paths)` 生成注入子进程的两个环境变量。

## 6. 与 TS 的已知差异

| 差异 | 说明 |
| --- | --- |
| 同步 legacy 导入 | TS 为 async；Rust 注入同步闭包，调用方自行封装异步读取。 |
| 定时线程而非 `setTimeout.unref` | Rust 用可停止的轮询线程；`dispose` 立即结束，不依赖运行时事件循环。 |
| 原子写临时文件 | 复用 `atomic-file`（与 TS 同一套 rename 重试、锁语义、0600 私有写）。 |

## 7. 验收

- `cargo test -p provider-config-repository`：11 条用例覆盖缺失/无效/非规范/legacy 导入、
  update 往返、锁与错误语义、运行时路径组合，以及轮询变更/单次错误上报/dispose 停止。
- 对拍：读结果 revision 与 `provider-config-source` 的 `read_personal_layer` 一致；
  规范写回文本与 `serde_json::to_string_pretty(encodeProviderConfigFile(update))` 一致。
- 真机/频道接管未开始；Node 仓库仍是 oracle。
