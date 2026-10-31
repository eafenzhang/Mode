# Spec：技能服务（Rust `skills-service`）

状态：2026-10-29 首版；2026-10-31 验收落地——`tests/replay.rs` 按 §11 用
`fixtures/skills.json` 回放五个用例，六个方法结果与结束树逐项深比较通过；
回放发现并修复 `publishedAt` 被渲染为浮点（`1700000000.0`）的差异，现与
TS `JSON.stringify` 的整数输出一致。Rust 侧只覆盖 **技能发现 / 开关 /
提示词上下文 / 通用目录管理**；
`skill-sync`（远端同步候选、归档导出导入）与 `mcp-sync` / `plugin-sync` 不在本 spec 内。

## 1. 范围

`crates/skills-service` 对齐 `packages/services/src/skills/skillsService.ts` 的六个方法：

| Rust                   | 真源                      | 说明                               |
| ---------------------- | ------------------------- | ---------------------------------- |
| `list`                 | `skillsService.ts` `list` | 扫描四类根 + 插件根，合并 CLI 开关 |
| `set_enabled`          | `setEnabled`              | 按 `SKILL.md` 路径写 CLI config    |
| `build_prompt_context` | `buildPromptContext`      | `$skill` 命中注入 + 审计日志       |
| `copy_to_common`       | `copyToCommon`            | 复制到 `<root>/.mode/skills/<dir>` |
| `remove_from_common`   | `removeFromCommon`        | 从通用目录删除                     |
| `delete_skill`         | `deleteSkill`             | 带受控根越界校验的删除             |

不在本 spec 内：频道注册的开关（壳侧 `MODE_HOST_INPROCESS_CHANNELS` 决定是否接管；
适配器 `crates/host-channels/src/skills.rs` 与注册已随本 crate 落地，默认不接管）、
`skill-sync`（依赖远端同步写权限与归档）、`grantWorkspaceHookTrust` 类 Agent authority。

## 2. 行为真源

- `packages/services/src/skills/skillsService.ts`（主体）
- `packages/services/src/skills/skillDiscoveryWalk.ts`（有界遍历）
- `packages/shared/src/skill-scan-policy.ts`（排除目录、`.system` 白名单、深度 8）
- `packages/shared/src/plugin-marketplaces.ts`（官方市场 id 归一、默认启用插件集合）
- `packages/shared/src/node/officialPluginCache.ts`（官方缓存根新旧回退）
- `packages/services/src/plugins/installedPluginRoots.ts`（`installed_plugins.json`）

## 3. 路径与根

`home_dir` 由调用方传入（壳内取 `resolveUserHomeDir()` 的结果）。TS 的
`resolveUserHomeDir()` = `HOME.trim() || USERPROFILE.trim() || homedir()`；Rust
`SkillsServiceOptions::from_process()` 复刻该顺序，其余场景显式注入，便于测试。

| 作用域    | 根                                             |
| --------- | ---------------------------------------------- |
| workspace | `<ws>/.mode/skills`、`<ws>/.agents/skills`     |
| user      | `<home>/.mode/skills`、`<home>/.agents/skills` |
| plugin    | 见 §7                                          |

祖先链（`resolveAncestorWorkspaceRoots`）：从 `workspacePath` 向上找第一个含 `.git` 的目录；
找到时从 `workspacePath` 逐层向上收集到该目录（含），每层**先 `.mode/skills` 后 `.agents/skills`**；
找不到 `.git` 时只用 `workspacePath` 自身。向上终止条件：`current === worktreeRoot` 或
`parent === current`。

扫描根归并（顺序不可交换）：

1. `dedupeRoots`：按字符串去重，保留首次出现顺序。
2. `filterNestedScanRoots`：只在**已存在**的根之间剔除「被更深的已存在根完整覆盖」的祖先
   （`normalizeScanRootPath` 把 `\` 转 `/` 并去尾 `/`；`other.startsWith(current + "/")`）。
   子目录不存在时保留父目录。
3. `dedupeScanRootsByRealpath`：存在路径取 `realpath` 作去重键，**返回原路径**。

## 4. 有界遍历（`walkSkillMarkdownPaths`）

显式栈 DFS（后进先出）：

- 每个目录先 `readdir`；失败 → 调 `onError` 后跳过该目录（调用方记 `skill_scan_failed`）。
- 名字恰为 `SKILL.md` 且不是目录（含指向文件的软链）→ 记为技能文件，先产出
  `join(dir, "SKILL.md")`。
- 其余条目过 `shouldWalkSkillDirectoryEntry`：排除
  `node_modules dist build out target vendor coverage .cache .next .turbo .venv __pycache__`；
  点目录只放行 `.system`。
- `depth >= 8` 后不再下钻。
- 普通子目录按 `readdir` 顺序入栈；软链先 `stat` 确认是目录（失败 → `onError`），
  再按 `realpath` 去重后入栈（与普通目录共用 `visitedSymlinkTargets`）。
- `collectSkillMarkdownPaths` 用 `Set` 去重后按 `localeCompare` 升序返回。

## 5. frontmatter 解析（`readFrontmatter` 的可观测输出）

只产出 `hasFrontmatter` / `name` / `description` / `body` 四个字段；TS 的 `keys` 与 `parseOk`
在服务内没有下游消费者，Rust 不实现。

1. 换行归一：`\r\n`、`\r` → `\n`。
2. 匹配 `^---\n([\s\S]*?)\n---(?:\n|$)`：必须以 `---\n` 开头，闭合行必须是行首 `---`
   且其后为 `\n` 或文件结束（取**第一处**满足）。不匹配 → `hasFrontmatter=false`，
   `body = normalized.trim()`。
3. `splitFrontmatterAndLeakedBody`：从第 2 行起找第一行匹配 `^#{1,6}\s+\S`；
   命中则前半为 `metadataText`（`trimEnd`），后半为 `leakedBody`（`trim`）。
4. `body` = `[leakedBody, 头部之后的正文]` 中非空者用 `"\n\n"` 连接后 `trim`。
5. 宽松字段（`readLooseFrontmatterFields`）：逐行取首个 `:`，key 为 `:` 前 `trim`；
   `name` 取内联值 `trim` 后去掉首尾单个 `"`/`'`；`description` 在值以 `|`/`>` 开头时
   收集后续以空格或 `\t` 开头的行（每行去掉 1–2 个前导空白，`>` 用空格连接、`|` 用换行连接），
   否则取内联值；后出现者覆盖前者。
6. `hasYamlUnsafeLooseInlineField`：只看 `name`/`description`；内联值为空或以
   `"` `'` `|` `>` 开头则跳过；含 `": "` 视为不安全 → **直接**返回宽松结果。
7. 否则做严格解析；解析抛错或结果不是对象 → 返回宽松结果。
8. 严格成功 → `name = String(parsed.name) || loose.name`，`description` 同式（`||` 为 JS 真值判断）。

### 5.1 严格解析的实现边界

TS 用 npm `yaml` 包，Rust 不引入 YAML 依赖，改为**受支持子集**（`crate::frontmatter`）：

- 顶层必须是映射；`key: value` 行，key 允许单/双引号；
- 标量：plain（去尾注释 ` #` 起）、单引号、双引号（`\n \t \\ \" \uXXXX` 转义）；
- 块标量 `|` `>` 及其 chomping/indent 指示符；
- 嵌套（缩进行）、流式 `[...]` / `{...}`、多行 plain 折叠 → 记录为**非字符串值**，
  按 JS `String()` 语义折算（对象 → `[object Object]`，标量数组 → 逗号连接）；
- 多文档、锚点/别名/标签、tab 缩进、重复 key、无 `:` 的顶层行 → **严格解析失败**（回退宽松）。

子集外的输入按「严格失败 → 宽松」处理，与 TS 在 YAML 报错时一致；与 TS 可能不同的
只剩「TS 严格解析成功、Rust 判失败」这一类，其 `name`/`description` 通常仍与宽松读取一致。
对拍用例只覆盖上述子集（见 §11）。

## 6. 开关映射（CLI config）

- 配置文件：`<home>/.mode/cli/config.json`；非对象（含数组）→ `{}`。
- 读：只收 `skills[path] = { enable: boolean }`，键做 `\`→`/` 归一（不 trim、不转小写）。
- 写：`enable === true` → **删除**该键（默认态不落盘）；否则写 `{ enable: false }`。
  归并后空对象则删除 `skills` 字段。按 key `localeCompare` 升序处理；目录
  `<home>/.mode/cli` 先 `mkdir -p`；文件内容 `JSON.stringify(config, null, 2) + "\n"`。
- `setEnabled` 串行化：同一服务实例内多次调用互不交叉（Rust 用互斥锁复刻）。

## 7. 插件技能根

CLI config 的 `plugins` 段：

```
dirs: string[]                        // 内联插件目录（市场 id = "inline"，默认启用）
enabled: boolean（缺省 true）          // false → 不贡献任何插件技能
enabledPlugins: { [id]: boolean }     // 键按 canonicalPluginId 归一
suppressedBuiltins: string[]          // 同样归一；官方市场命中即跳过
storageDir: <config.storage.dir> 或 "~/.mode"
```

路径解析：`~` 展开为 home（`"~/"` 前缀，`join(home, rest)`）；非绝对路径 `resolve()`；
`resolveCliStorageRoot` 在 basename 不是 `cli` 时补 `cli`；插件存储根再补 `plugins`。

候选顺序（`resolvePluginSkillRootDescriptors`）：

1. `config.dirs`（`defaultEnabled: true`，marketplace `inline`）；
2. 官方缓存：`<plugins>/cache/mode-plugins-official`，不存在则回退
   `<plugins>/cache/zcode-plugins-official`，再不存在仍用新路径（扫描得空）。
   本地版扫描**不过滤**备份/临时目录、**不按数字排序**，只取两层目录并在最后按路径
   `localeCompare` 升序（`defaultEnabled: false`，marketplace `mode-plugins-official`）；
3. `installed_plugins.json`：`plugins[]` 每项须 `id`/`marketplace`/`installPath` 为非空字符串且
   `installPath` 绝对；`id` 走 `canonicalPluginId`，`marketplace` 取归一 id 的 `@` 后半段
   （为空时回退原值）→ `defaultEnabled: false`。

每个候选：

- 清单按序找 `.mode-plugin/plugin.json`、`.zcode-plugin/plugin.json`、
  `.claude-plugin/plugin.json`、`.codex-plugin/plugin.json`（第一个存在的）；
  读取失败/JSON 非法/非对象 → 跳过；
- `name` 必须 `trim` 后匹配 `^[a-z0-9][a-z0-9._-]{0,127}$`，否则跳过；
- `pluginId = "<name>@<marketplace>"`；
- 官方市场且命中 `suppressedBuiltins` → 跳过；
- `seenPluginIds` **先 add 再判 enabled**（重复 id 的后一个候选不参与）；
- `defaultEnabled = 候选默认值 || isDefaultEnabledOfficialPluginId(pluginId)`；
  `enabled = config.enabledPlugins[pluginId] ?? defaultEnabled`；false → 跳过；
- 技能根：`skills` 字段为字符串或字符串数组，每项经 `resolveInside`（拒绝绝对路径与越界）；
  **仅当结果为空且 `skills` 字段为 undefined** 时回退 `<root>/skills`（需存在）。

`canonicalPluginId` 只把 `@zcode-plugins-official` 后缀改写为 `@mode-plugins-official`。
默认启用的官方插件 id：`browser-use image-search documents pdf presentations spreadsheets
node-repl-host skill-creator plugin-creator mode-guide`（均 `@mode-plugins-official`；
`computer-use` 默认关闭）。

## 8. 发现与合并（`discoverSkills`）

1. workspace 根 → `scope: workspace`；`includeUserSkills` 时追加用户两根（`scope: user`）；
   再追加插件描述符（`scope: plugin`，带 `pluginName`/`pluginId`）。
2. `scanRootPaths = dedupeScanRootsByRealpath(所有 rootPath)`；按该顺序处理，
   根不存在则跳过。
3. 每个根收集 `SKILL.md`（§4）。逐条：
   - `isUserAgentsSkillCoveredByMode`：仅当根**字符串等于**用户 `.agents/skills` 时生效；
     用户 `.mode/skills/<dir>/SKILL.md` 存在 → 跳过；否则按 name key（`trim().toLowerCase()`）
     落在用户 `.mode` 根已收集的 name key 集合中 → 跳过。
   - `canonicalSkillPath = realpath(skillPath) || skillPath`，按它做全局去重（先到先得）。
   - 再次 `exists(skillPath)` 为假 → 跳过。
   - 读取失败 → 诊断 `skill_read_failed`（message 取错误文本）后跳过。
   - `name`：有 frontmatter 用 `parsed.name.trim()`，否则用父目录名；两者都空 → 诊断
     `skill_missing_name` 后跳过。
   - `description`：有 frontmatter 用 `parsed.description.trim()`，否则 `""`；
     长度 > 1024 → 诊断 `skill_description_too_long`（带 `skillName`）后跳过。
   - `body = parsed.body.trim()`；`metadata` 取同目录 `_meta.json`（§8.1）。
   - push：`id = glm:<scope>:<name>:sha256(path)[0..12]`，`path` 用 realpath 结果，
     `sourcePath` 用未解析路径，`enabled: true`（开关在 `list` 阶段覆盖），
     可选 `pluginName`/`pluginId`/`metadata`。
4. 排序：`name` → `scope` → `path`，全部 `localeCompare`。

### 8.1 `_meta.json`

读取失败或空内容 → 无 metadata；非对象 → 无；只收 `slug`/`version`/`ownerId`
（string 且 `trim()` 非空，存 trim 值）与 `publishedAt`（number 且有限）；全空 → 无；
JSON 异常 → 无。

## 9. 方法语义

- `list`：`provider` 恒为 `"glm"`；`capability.userScopeAvailable` 由
  `isDesktopRuntime ?? Boolean(MODE_PROCESS_LABEL)` 决定，true 时无 `userScopeReason`，
  false 时 `userScopeReason: "desktop_only"` 且不扫描用户根。最后按
  `enabledByPath[归一 path] ?? true` 覆盖 `enabled`。
- `set_enabled`：重新发现后用 `id` 找技能；找不到抛 `Skill not found: <id>`；
  写开关（§6）。
- `build_prompt_context`：用 `/\$([a-z0-9]+(?:-[a-z0-9]+)*)/g` 收集提及名；空集直接原样返回。
  重新 `list` 后筛 `enabled && 提及`；为空原样返回。否则**先**追加审计日志再返回
  `` `${prompt}\n\n${block}` ``，其中 block 为：

  ```
  <available_skills>
  <activated_skill name="<转义 name>" path="<转义 path>">
  <body>
  </activated_skill>
  </available_skills>
  ```

  转义顺序 `&` `"` `<` `>`。审计日志：`<home>/.mode/v2/skills-audit.log`，每行
  `{createdAt: Date.now(), workspacePath, workspaceIdentity|null, activatedSkillNames}`。

- `copy_to_common`：目标根按技能原 scope 选 workspace 或 user 的 `.mode/skills`；
  目标目录已存在 → 抛 `通用目录已存在同名技能: <basename>`；否则 `mkdir -p` 后递归复制，
  返回 `<target>/SKILL.md`。
- `remove_from_common`：把 `skill.path`、两个 `.mode/skills` 根都转小写正斜杠后判断包含
  （`<root>/`）；都不在 → 抛 `该技能不在通用目录中`；否则删除 `dirname(skill.path)`。
- `delete_skill`：`scope === plugin` → 抛 `插件提供的技能不可单独删除，请卸载对应插件`；
  用 `sourcePath ?? path` 的父目录定位；只对**父目录**做 realpath，失败 → 抛
  `该技能不可删除: <path>`；父目录必须落在（或等于）祖先链两个根 + 用户两根之一的
  realpath 内，否则同样抛出；最后删除 `<real_parent>/<leaf>`（软链叶子只删链接）。

## 10. 已知差异（不与 TS 逐字节对齐的部分）

1. **排序**：TS 用 ICU `localeCompare`（默认 locale），Rust 用「Unicode 小写折叠 + 大小写
   次序」近似。ASCII 字母、数字、`-`/`_`/`.`/`/` 组合一致；非 ASCII 与 ICU 特殊权重
   （变音符号、忽略标点）不保证。
2. **JSON 对象键序**：TS `JSON.stringify` 保留读入顺序，Rust `serde_json` 用有序 map，
   写回的 CLI config 键序可能不同；语义（键值集合）一致。
3. **YAML 严格解析**：见 §5.1 子集边界。
4. **文件解码**：Rust 用 `String::from_utf8_lossy`，与 Node 的 UTF-8 替换字符行为在
   截断多字节序列的粒度上可能差一个字符。
5. **符号链接**：`cp -r` 的软链语义按平台尽力而为；对拍用例只覆盖普通文件与目录。

## 11. 验收

- `pnpm skills:fixtures` 生成 `crates/skills-service/fixtures/skills.json`
  （TS 真源执行 + 归一化 `<root>`）。
- `cargo test -p skills-service` 用同一份文件树重放并逐项深比较：
  `list` / `set_enabled` / `build_prompt_context` / `copy_to_common` /
  `remove_from_common` / `delete_skill` 的结果与服务结束后的整棵树。
- 两处生成结果 `sha256` 必须一致（脚本可重复执行）。
