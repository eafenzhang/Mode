# Spec：技能归档同步（Rust `skill-sync-service`）

状态：2026-10-31 首版。对应 `packages/services/src/skill-sync/` 的
`ISkillSyncService`（5 个方法）。

## 1. 范围

| Rust                                   | 说明                                                 |
| -------------------------------------- | ---------------------------------------------------- |
| `list_local_user_skill_candidates`     | 用户级技能候选（mode + agents 两根、覆盖去重、大小） |
| `list_remote_user_skill_statuses`      | 按目录名/技能名检查本地是否已存在                    |
| `export_skills_archive`                | 选中候选 → ustar + gzip 归档                         |
| `check_remote_user_skill_write_access` | 在用户 skills 根写标记探测                           |
| `import_skills_archive`                | 解归档 → 校验 SKILL.md → 复制到用户 skills 根        |

**频道已注册**：导出结果里的 `archive` 是嵌套 `Uint8Array`。rpc-wire 不做透明还原，
适配器使用共享辅助 `rpc_wire::nested_bytes` 在返回时构造标记、导入时还原为字节
（判定与 TS 的 `isRpcEncodedUint8Array` 同口径）。`skill-sync` 频道已注册进
`MODE_HOST_INPROCESS_CHANNELS`（默认不接管）。

## 2. 候选扫描

- 根：`{home}/.mode/skills`（mode）与 `{home}/.agents/skills`（agents）。
- 复用 `skills_service::walk_skill_markdown_paths`（排除目录、点目录白名单、
  深度 8、软链 realpath 去重）。
- 每个 `SKILL.md`：`directoryName` = 相对根的 `/` 分隔路径；`name`/`description`
  从 frontmatter 读取（受支持 YAML 子集；失败或缺失回落目录名 / 空串）；
  `path` = SKILL.md 绝对路径；`id` = `sha256(directoryPath)`；
  `sizeBytes` = 目录递归大小（软链跟随，真实路径去重防环）。
- 覆盖规则（与 skills 服务一致）：mode 根下含 `SKILL.md` 的目录名集合；agents 根里
  同名目录、或技能 name（trim + 小写）已在 mode 候选中出现 → 排除。再按
  `realpath(SKILL.md)` 全局去重。最后按 `directoryName` localeCompare 升序。
- `maxArchiveBytes` 默认 20 MiB（可注入覆盖）。

## 3. 归档格式

- **ustar + gzip**，只含普通文件（typeflag `0`）与目录（`5`）：
  - 目录条目路径带尾 `/`，size 0；文件按 512 字节块对齐并以 0 填充；
  - header 字段与 TS 逐字节一致：mode（`& 0o777`）、uid/gid 0、size/mtime 八进制、
    checksum 六位八进制 + `\0` + 空格、magic `ustar`、version `00`、
    uname/gname `mode`；长路径按 `prefix`(≤155)/`name`(≤100) 拆分；
  - 目录子项按名称 localeCompare 排序；软链等非普通文件导出直接报错
    （`unsupported skill archive source`）。
- 导出顺序：选中候选的 `directoryName`；归档路径即 `directoryName`。
- 大小门禁：选中内容总和与归档体积分别对照 `maxArchiveBytes`，错误文案
  `skill sync size limit exceeded: <actual>/<max> (<phase>)`。
- 导入：gzip 解压后按块解析；目录条目建目录，文件条目累计字节数（超过
  `maxArchiveBytes` 判 `extracted-content` 超限）、校验路径安全后写盘；
  非 `0`/`5` 类型报 `unsupported skill archive entry type: <type>`。

## 4. 导入流程

1. `overwrite === true` → `skill sync overwrite is not supported`。
2. 归档体积超限 → `archive` 阶段错误。
3. 解到 `{user mode skills}/.sync-tmp-<pid>-<nanos>`，结束时删除。
4. 收集含 `SKILL.md` 的目录（有界策略，按 `directoryName` 升序）。
5. 每个目录：缺 `SKILL.md` → `failed: SKILL.md is missing`；目标已存在 → `skipped`
   （`targetExists`）；同名技能已在 mode/agents 根存在 → `skipped`
   （`sameNameExists`，path 指向已有目录）；否则递归复制（目标存在即失败）→ `synced`，
   并把新名字加入去重集合。

## 5. 已知差异

1. **gzip 字节**：Node zlib 与 flate2（miniz_oxide）压缩输出不同，归档字节不能逐字节
   比较；tar 结构、条目顺序与 header 字段一致，双向可解。
2. **文件 mode**：Windows 上 Rust 无 POSIX mode，写 `0o666`（文件）/`0o777`（目录）；
   Unix 取真实权限位。
3. **YAML**：只解析顶层标量；块标量（`|`/`>`）按空值处理（取不到 name/description 时
   回落），完整 YAML 语义不在本服务范围。
4. **错误文本**：gzip 损坏在 TS 是 stream 错误，Rust 统一为
   `invalid skill sync archive: …`。
5. **token/临时目录名**：TS 用 UUID，Rust 用进程号 + 纳秒；只影响临时目录名。

## 6. 验收

- `pnpm skill-sync:fixtures` 生成 `crates/skill-sync-service/fixtures/skill-sync.json`
  （候选/远端状态/写权限、导出元数据与错误、TS 归档在 Rust 侧导入并比较结束树；
  生成前把文件 mtime 固定，脚本可重复执行）。
- `cargo test -p skill-sync-service` 回放三个用例，逐项深比较结果与结束树
  （候选 id、archiveBytes 归一化）。
