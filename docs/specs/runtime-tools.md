# Spec：bundled 工具解析与 PATH 补丁（Rust `runtime-tools`）

状态：2026-11-02 首版。
真源：`packages/services/src/runtime-tools/runtimeToolResolver.ts`。

## 1. 范围

| Rust 入口                                | TS 对应                      |
| ---------------------------------------- | ---------------------------- |
| `prepend_path_entries` / `append_path_entries` | 同名函数               |
| `find_runtime_tool_binary`               | `findRuntimeToolBinary`      |
| `build_runtime_tool_env_patch`           | `buildRuntimeToolEnvPatch`   |
| `build_shell_bootstrap_path`             | `buildShellBootstrapPath`    |
| `resolve_shell_path_for_login_env`       | `resolveShellPathForLoginEnv` |
| `build_login_shell_args`                 | `buildLoginShellArgs`        |
| `build_login_shell_execution_options`    | `buildLoginShellExecutionOptions` |
| `extract_captured_env_snapshot` / `parse_null_separated_env_snapshot` | 同名私有函数 |
| `capture_login_shell_env_snapshot`        | `captureLoginShellEnvSnapshot` |
| `find_mode_agent_runtime_binary`          | `findModeAgentRuntimeBinary` |
| `find_mode_agent_runtime_node_bundle`     | `findModeAgentRuntimeNodeBundle` |
| `resolve_client_config_platform`          | `resolveClientConfigPlatform` |

文件系统探测通过 `ToolFileSystem` 注入（`exists` + `is_executable`）；生产使用
`NativeToolFileSystem`（Unix 检查 `0o111`，Windows 按 Node `access(X_OK)` 的存在性语义）。

## 2. 工具表

| ToolId    | binaryEnvVar      | bundledResourceDir | 二进制名（Windows 加 `.exe`） |
| --------- | ----------------- | ------------------ | ----------------------------- |
| `bfs`     | `MODE_BFS_BINARY` | `bfs`              | `bfs`                         |
| `ripgrep` | `MODE_RG_BINARY`  | `ripgrep`          | `rg`                          |
| `ugrep`   | `MODE_UGREP_BINARY` | `ugrep`          | `ugrep`                       |

## 3. 解析顺序

1. 同名 binary env（trim 非空且文件存在可执行）→ 原样返回（不做路径归一化）。
2. `MODE_SERVER_RUNTIME_ROOT` → `<root>/tools/<dir>/<binary>`。
3. `resourcesPath` → `<resources>/tools/<dir>/<binary>`。
4. 平台目录候选（按 cwd 与 moduleDir 展开）：
   `bundled-tools/<platform>-<arch>/<dir>/<binary>`、`packages/desktop/...`、
   `../desktop/...`，以及 moduleDir 相对的两个 `desktop/bundled-tools`。
5. PATH 回退：命令名去掉结尾 `.exe`；Windows 且命令不含 `.` 时用 `PATHEXT`
   （缺省 `.EXE/.CMD/.BAT/.COM`；显式空值按空列表处理），否则只试原名；
   逐 PATH 条目拼接并检查可执行。

命中后写 `<binaryEnvVar>`，把其目录追加到 PATH；PATH 追加按原顺序去重（保留首次出现），
分隔符 Windows `;` / POSIX `:`。Windows 上 `path.join/resolve` 的 `/` 会统一成 `\`
（与 Node 一致），env 原样返回的路径保持原分隔符。

## 3.1 POSIX login shell 采集（纯部分）

- bootstrap PATH：Linux 与 darwin 各自默认系统目录前置到当前 PATH；分隔符用**进程平台**
  （win32 上是 `;`，POSIX 是 `:`），与 TS `prependPathEntries` 一致。
- shell 选择：`$SHELL` → `/bin/zsh` → `/bin/bash` → `/bin/sh`，逐项检查存在且可执行。
- 参数：路径以 `/zsh`/`/bash` 结尾用 `-ilc`，否则 `-lc`；命令固定输出
  `__MODE_LOGIN_ENV_START__\0` + `env -0` + `__MODE_LOGIN_ENV_END__\0`。
- 执行选项：base env 上覆盖 PATH（bootstrap）、`TERM=dumb`、`CI=1`；默认 timeout 4000ms，
  maxBuffer 2 MiB。
- 解析：取最后一对标记之间的 NUL 分隔输出；key 需匹配 `[A-Za-z_][A-Za-z0-9_]*`，
  值保留首个 `=` 之后的全部内容。

捕获层 `capture_login_shell_env_snapshot` 由调用方注入 `LoginShellExecutor`，负责
win32/VITEST/shellPath=null 短路、deadline（tokio timeout）与异常吞掉后返回 null；
`process_platform` 单独传入以复刻 TS 中 `process.platform` 与 `options.platform` 的差异
（真实调用两者相同）。默认子进程执行与同步缓存仍由调用方实现。

## 3.2 Agent 运行时资源查找

候选顺序（原生二进制 `glm/mode-agent[.exe]` 与 JS bundle `glm/mode.cjs` 共用）：

1. `GLM_BINARY_PATH`（仅二进制；非空且存在 → 原样返回）。
2. `resourcesPath/glm/<entry>`。
3. `~/.mode/server/agents/glm/<entry>`。
4. cwd 平台目录：`bundled-agents/<platform>-<arch>`、`packages/desktop/...`、`../desktop/...`，
   以及 moduleDir 相对的两个目录。
5. 旧布局：cwd 的 `bundled-resources`、`packages/desktop/bundled-resources`、
   `../desktop/bundled-resources`，以及 moduleDir 相对的两个目录。

只看存在性（目录也算），返回第一个命中；不做可执行位检查。

## 3.3 客户端配置平台标识

`MODE_TEST_CLIENT_CONFIG_PLATFORM` 优先（trim 非空）；否则由
`MODE_TEST_RELEASE_TARGET`（缺省 darwin/linux/win32 → darwin/linux/windows，其它平台原样）
与 `MODE_TEST_RELEASE_ARCH`（缺省 x64/arm64 → x86_64/aarch64，其它架构原样）拼成
`<target>-<arch>`。

## 4. 已知差异 / 边界

1. **平台夹具**：TS 的 `process.platform/arch` 不可注入，夹具只覆盖 win32-x64；
   POSIX 分支依赖 Rust 原生 `std::path`，不在此夹具口径内。
2. `moduleDir` 在 TS 是模块自身目录；Rust 调用方需传入等价目录（本夹具未覆盖模块候选，
   只用 cwd/runtimeRoot/环境变量/PATH 路径）。
3. `resourcesPath` 在 Node 下缺省 undefined；Rust 用 `None` 表示，空串按 cwd 相对解析。
4. 目录也会通过 Windows 的「可执行」检查（Node `X_OK` 语义），与 TS 一致。

## 5. 验收

- `pnpm runtime-tools:fixtures` → `crates/runtime-tools/fixtures/runtime-tools.json`
  （11 条：runtime root、cwd/packages/兄弟目录 bundled、env 覆盖与失效回退、
  PATH 默认/PATHEXT 回退、PATH 去重、多工具、全部缺失；4 条 bootstrap PATH；
  8 条 login shell：显式/解析 shell、非法 env key、无标记、最后一对标记、自定义 timeout；
  10 条 agent 运行时：env 覆盖、平台目录/旧布局、bundle、空 env 回退、全部缺失；
  8 条客户端平台标识：三平台映射、未知平台/架构、三个覆盖与空白覆盖；
  7 条异步捕获：成功、win32/VITEST/null 短路、executor 抛错、无标记、5ms deadline）。
- `cargo test -p runtime-tools`（Windows 上逐项深比较；非 Windows 直接跳过）。
- `cargo clippy -p runtime-tools --all-targets -- -D warnings`。
