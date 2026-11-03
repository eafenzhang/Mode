# Spec：Mode Built-in 缓存路径与随包基线物化（Rust）

> 真源：`packages/provider-node/src/mode-builtin-cache-paths.ts` 与
> `mode-builtin-provider-config-materializer.ts`。
> 所在 crate：`crates/provider-config-repository`（`cache_paths` / `materializer`）。
> 只读文件层的 Active/Bundled 选择见 `provider-config-source.md`。

## 1. 范围与所有者

| Rust 入口 | TS 对应 | 说明 |
| --- | --- | --- |
| `cache_paths::resolve_mode_builtin_client_platform` | `resolveModeBuiltinClientPlatform` | 平台/架构段映射，纯函数 |
| `cache_paths::resolve_mode_builtin_cache_paths` | `resolveModeBuiltinCachePaths` | Active/LKG 路径布局 |
| `cache_paths::create_mode_builtin_endpoint_key` | `createModeBuiltinEndpointKey` | Origin 规范化 + sha256 前 32 位 |
| `materializer::materialize_mode_builtin_provider_config` | `materializeModeBuiltinProviderConfig` | 随包 Release 的唯一写入点 |

- **路径是兼容范围**：`runtime/provider/<platform>/<appVersion>/endpoint-<hash>/` 的层级与文件名
  必须与 TS 一致；平台或版本段非法时拒绝，不做路径清洗。
- **唯一写入点**：随包基线只由 materializer 写入；它复用 `atomic-file` 的跨进程锁与
  私有原子写，其他层不得直接写该文件。
- materializer 与下载配置分离，不保留历史 hash 副本；内容未变化时不重写。

## 2. 路径规则

```text
<environmentConfigRoot>/runtime/provider/<platform>/<appVersion>/<endpointKey>/
  ├─ mode-builtin.json           // Active
  └─ mode-builtin-refresh.json   // 刷新控制面
```

- `platform` 段：TS `process.platform` 映射 `win32 → windows`，其余保持；架构
  `x64 → x86_64`、`arm64 → aarch64`、`ia32 → ia32`；组合为 `<platform>-<arch>`。
- `appVersion`、`platform` 先 trim；空、`.`、`..` 或含 `/`、`\` 时返回
  `Mode Built-in <name> 不是合法路径段`。
- Endpoint Origin：trim 后必须能解析为 HTTP(S)；否则
  `Mode Built-in Endpoint Origin 只支持 HTTP(S)` / `不能为空`。key 为规范 Origin 的
  sha256 十六进制前 32 位，前缀 `endpoint-`。

## 3. 物化规则

```text
materialize({ environmentConfigRoot, content })
  ├─ JSON.parse + decodeModeBuiltinRelease（严格 schema）
  ├─ serializeModeBuiltinRelease + 追加 "\n"
  ├─ 建父目录 → withFileLock
  ├─ 读取现有文件；内容相同 → 不写
  └─ 不同 → atomicWritePrivateTextFile（0600）
```

- 目标路径：`<environmentConfigRoot>/runtime/provider/bundled/mode-builtin.json`。
- 非法 JSON / Release schema 错误直接返回，不写文件；已有文件保持不变。
- 首次物化时父目录不存在：先 `create_dir_all`，否则锁目录无法创建。
- 返回写入的目标路径。

## 4. 失败语义

| 场景 | 行为 |
| --- | --- |
| 平台/版本段非法 | 错误，不产出路径 |
| Origin 为空 / 非 HTTP(S) | 错误，不产出 key |
| 内容不是 JSON 或 Release 非法 | 错误，不建目录、不写文件 |
| 父目录创建失败 / 锁超时 / 原子写失败 | 错误原样上抛，交由调用方重试 |
| 内容与现有文件一致 | 跳过写入，返回路径 |

## 5. 与 TS 的已知差异

| 差异 | 说明 |
| --- | --- |
| 同步 API | TS 为 async；Rust 为同步，调用方在异步边界用阻塞任务包装。 |
| 平台/架构注入 | 生产用 `resolve_mode_builtin_client_platform_from_process`；测试可直接传入平台/架构。 |
| 路径比较 | 复用 `atomic-file` 的锁与写；Windows 分隔符由 `PathBuf` 处理。 |

## 6. 验收

- `cargo test -p provider-config-repository`：5 条缓存用例覆盖平台映射、Origin 归一、
  路径布局与非法段、首次物化/内容不变不重写、嵌套目录创建；仓库其余 11 条用例同时回归。
- 真机/远程同步未开始；Node 物化路径仍是 oracle。
