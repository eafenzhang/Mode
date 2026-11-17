//! 项目记忆召回（C2 T3）：`memoryEnabled` 开启时把项目记忆索引注入模型上下文。
//!
//! 行为真源：`docs/specs/agent-c2-contract.md` §3（T0 机器录制）与夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/memory-recall.json`
//! （只读数据契约，不修改）。Clean-room 纪律：本模块的注入文案**逐字重建**自该
//! 夹具录制的观察值（与 C2 T2 摘要续接文本同一口径：落库/上下文数据契约，不是
//! 提示词创作），路径与时点用真实事实替换。
//!
//! # 事实与判定
//!
//! - 目录：`<MODE_DATA_BASE_DIR>/.mode/cli/memories/projects/<workspaceId>/memory/`；
//!   入参 `data_root_mode` 就是 `.mode` 根（Host `resolveDataRoot` 形态，与
//!   [`crate::state::RuntimeConfig`] 的 `data_root` 同源）。
//! - `workspaceId` 形状 `<slug>-<16 位小写 hex>`，`slug` 是 cwd 基名的小写化；
//!   **hash 算法未观察**（契约 §3.1），因此不推导——扫描 `projects/` 下匹配
//!   `<slug>-<16 hex>` 的普通目录，恰好一个命中才使用，零/多个命中一律 fail-soft
//!   （不注入）。
//! - 索引优先：只读 `MEMORY.md`；item 文件（如 `item-one.md`）**不**进入主请求
//!   （夹具 `containsItemSentinel:false`）。
//! - 读取走 [`project_memory::ProjectMemoryService`] 的公开 API：普通目录/文件、
//!   无 symlink、大小写精确、5 MiB 上限都在该层强制；任何失败（目录缺失、workspaceId
//!   歧义、文件不可读、超限）fail-soft——不注入、turn 继续，不把错误写进模型上下文。
//! - `memory_enabled == false` 时**零读取**（spec §5.1）：不触碰文件系统。
//!
//! # 与 oracle 观察的偏差（登记在 `agent-c2-contract.md` §3.5）
//!
//! - oracle 在 `memoryEnabled:true` 启用时会**创建**记忆目录；本实现只读不创建，
//!   目录不存在即视为无记忆。
//! - 注入文案中的路径分隔符按真实文件系统（Windows 观察为 `\`，`# Memory` 段的
//!   路径带尾随 `/`）。
//! - 记忆写路径（memory extraction 子代理）**未实现**：T0 只观察到请求形状，没有
//!   观察到真实写入；C2 不新增工具、不实现抽取子代理（契约 §3.3 的缺口登记）。

use std::path::{Path, PathBuf};

use agent_core::context::MemorySection;
use project_memory::ProjectMemoryService;

use project_memory::{PROJECT_MEMORY_DIRECTORY_NAME, PROJECT_MEMORY_INDEX_FILE_NAME};

/// workspaceId 的 hash 长度（观察值：16 位小写十六进制；算法未观察）。
const WORKSPACE_HASH_LENGTH: usize = 16;

/// `# Memory` 系统段模板（夹具 `withMemory.requests[0].memorySection` 的逐字录制，
/// 路径折叠位换成 `<memory-dir>`；结尾的双换行是录制文本的一部分）。
const MEMORY_SECTION_TEMPLATE: &str = "\
# Memory

You have a persistent file-based memory at `<memory-dir>/`. This directory already \
exists — write to it directly with the Write tool (do not run mkdir or check for its \
existence). Each memory is one file holding one fact, with frontmatter:

```markdown
---
name: <short-kebab-case-slug>
description: <one-line summary — used to decide relevance during recall>
metadata:
  type: user | feedback | project | reference
---

<the fact; for feedback/project, follow with **Why:** and **How to apply:** lines. \
Link related memories with [[their-name]].>
```

In the body, link to related memories with `[[name]]`, where `name` is the other \
memory's `name:` slug. Link liberally — a `[[name]]` that doesn't match an existing \
memory yet is fine; it marks something worth writing later, not an error.

`user` — who the user is (role, expertise, preferences). `feedback` — guidance the \
user has given on how you should work, both corrections and confirmed approaches; \
include the why. `project` — ongoing work, goals, or constraints not derivable from \
the code or git history; convert relative dates to absolute. `reference` — pointers \
to external resources (URLs, dashboards, tickets).

After writing the file, add a one-line pointer in `MEMORY.md` (`- [Title](file.md) — \
hook`). `MEMORY.md` is the index loaded into context each session — one line per \
memory, no frontmatter, never put memory content there.

Before saving, check for an existing file that already covers it — update that file \
rather than creating a duplicate; delete memories that turn out to be wrong. Don't \
save what the repo already records (code structure, past fixes, git history, \
AGENTS.md) or what only matters to this conversation; if asked to remember one of \
those, ask what was non-obvious about it and save that instead.

";

/// agentsMd system-reminder 的固定头部（夹具 `memoryFileBlock` 的逐字录制，索引路径
/// 折叠位换成 `<memory-index-path>`）。
const AGENTS_MD_HEADER: &str = "\
<system-reminder>
As you answer the user's questions, you can use the following context:
# agentsMd
Codebase and user instructions are shown below. Be sure to adhere to these \
instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you \
MUST follow them exactly as written.

Contents of ";

/// agentsMd system-reminder 的固定尾部（同上；`<date>` 是折叠位，格式未观察）。
///
/// 注意开头有一个空格（`MEMORY.md (user's ...)`），不能用 `\` 续行——它会吃掉
/// 下一行的前导空白。
const AGENTS_MD_TAIL_BEFORE_DATE: &str =
    " (user's auto-memory, persists across conversations):\n\n";

/// 一次成功加载的项目记忆上下文：目录事实 + 已读取的索引内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryContext {
    /// 记忆目录绝对路径（`<data-root>/.mode/cli/memories/projects/<workspaceId>/memory`）。
    memory_dir: PathBuf,
    /// 命中的 workspaceId（`<slug>-<16 hex>`；扫描发现，不推导）。
    workspace_id: String,
    /// `MEMORY.md` 索引内容（原样注入 agentsMd 块；item 文件不读）。
    index_content: String,
}

impl MemoryContext {
    /// fail-soft 加载入口：关闭即零读取；任何失败返回 `None`（不注入、turn 继续）。
    ///
    /// `data_root_mode` 是 `.mode` 数据根（[`RuntimeConfig::data_root`] 同源）；
    /// `cwd` 是会话工作目录（slug 来源）；`memory_enabled` 来自宿主对
    /// `session/requestRuntimePreferences` 的应答（[`crate::reverse::RuntimePreferences`]），
    /// 不是常量。
    pub fn load(data_root_mode: &Path, cwd: &str, memory_enabled: bool) -> Option<Self> {
        if !memory_enabled {
            return None;
        }
        let slug = workspace_slug(cwd)?;
        let projects_root = data_root_mode.join("cli").join("memories").join("projects");
        let workspace_id = resolve_workspace_id(&projects_root, &slug)?;
        // 索引读取走公开服务：普通文件/无 symlink/大小写精确/5 MiB 全部由它强制；
        // 目录缺失、文件缺失、超限都归入 fail-soft。
        let index = ProjectMemoryService::new(data_root_mode)
            .read_project_memory_file(&workspace_id, PROJECT_MEMORY_INDEX_FILE_NAME)
            .ok()?;
        Some(Self {
            memory_dir: projects_root
                .join(&workspace_id)
                .join(PROJECT_MEMORY_DIRECTORY_NAME),
            workspace_id,
            index_content: index.content,
        })
    }

    /// 命中的 workspaceId（诊断/测试观察用；不出现在模型上下文之外）。
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// 记忆目录绝对路径（`# Memory` 段里展示给模型的同一事实）。
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// 系统提示的 `# Memory` 段正文（模板 + 真实目录路径）。
    ///
    /// 路径分隔符是运行平台的原生形状（夹具在 Windows 上录制为 `\`），`# Memory`
    /// 段的路径按观察带尾随 `/`。
    pub fn system_section(&self) -> String {
        MEMORY_SECTION_TEMPLATE.replace("<memory-dir>", &self.memory_dir.to_string_lossy())
    }

    /// 首条 user 消息前缀的 agentsMd system-reminder 块（索引内容 + `# currentDate`）。
    ///
    /// `today` 是 `YYYY-MM-DD` 日期串（[`today_utc`] 产出）；索引内容原样拼接，
    /// 块以 `</system-reminder>\n` 结尾（与录制一致）。
    pub fn agents_md_block(&self, today: &str) -> String {
        let index_path = self.memory_dir.join(PROJECT_MEMORY_INDEX_FILE_NAME);
        let mut block = String::with_capacity(
            AGENTS_MD_HEADER.len()
                + index_path.to_string_lossy().len()
                + AGENTS_MD_TAIL_BEFORE_DATE.len()
                + self.index_content.len()
                + 160,
        );
        block.push_str(AGENTS_MD_HEADER);
        block.push_str(&index_path.to_string_lossy());
        block.push_str(AGENTS_MD_TAIL_BEFORE_DATE);
        block.push_str(&self.index_content);
        block.push_str("\n# currentDate\nToday's date is ");
        block.push_str(today);
        block.push_str(
            ".\n\n      IMPORTANT: this context may or may not be relevant to your tasks. \
             You should not respond to this context unless it is highly relevant to your \
             task.\n</system-reminder>\n",
        );
        block
    }

    /// 装配成 [`MemorySection`]：`# Memory` 段进系统提示，agentsMd 块进首条 user 消息。
    pub fn to_memory_section(&self, today: &str) -> MemorySection {
        MemorySection::new(self.system_section()).with_user_prefix(self.agents_md_block(today))
    }
}

/// cwd 基名的小写化（观察值：slug = cwd basename lowercase）。
///
/// cwd 没有基名（空串/根）时返回 `None`（fail-soft，不猜 slug）。
fn workspace_slug(cwd: &str) -> Option<String> {
    let name = Path::new(cwd).file_name()?.to_string_lossy().into_owned();
    let slug = name.to_lowercase();
    (!slug.is_empty()).then_some(slug)
}

/// 扫描 `projects/` 下匹配 `<slug>-<16 hex>` 的普通目录；恰好一个命中才返回。
///
/// hash 算法未观察（契约 §3.1），这里只做形状匹配：前缀是 slug + `-`，余下恰好
/// 16 个小写十六进制字符。零命中（目录未创建）与多命中（同 slug 多目录）都返回
/// `None`——不猜、不取「最新」。
fn resolve_workspace_id(projects_root: &Path, slug: &str) -> Option<String> {
    let entries = std::fs::read_dir(projects_root).ok()?;
    let prefix = format!("{slug}-");
    let mut matches: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(hash) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !is_workspace_hash(hash) {
            continue;
        }
        // 与 project-memory 同一安全口径：只认普通目录，symlink 与其它类型跳过。
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        matches.push(name);
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// 16 位小写十六进制（观察形状；大写与其它长度不匹配）。
fn is_workspace_hash(value: &str) -> bool {
    value.len() == WORKSPACE_HASH_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `# currentDate` 的日期串：UTC 日历日 `YYYY-MM-DD`。
///
/// 夹具把该值折叠为 `<date>`，格式未观察（契约 §3.5 登记）；这里沿用仓库其他时间
/// 事实的 UTC 口径，不冒充观察值。
fn today_utc() -> String {
    let days = now_ms().div_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Unix 日数 → 公历日期（与 `data-root`/`usage-stats` 内部同族的民用历算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 本 turn 的项目记忆段：运行时每 turn 调用；关闭或任何失败都返回 `None`。
///
/// 读取是同步文件 IO，调用方必须已在阻塞线程上（[`crate::runtime`] 的
/// `spawn_blocking` turn 执行体）。诊断只在本模块内吞掉——失败不写日志内容之外
/// 的敏感信息（路径已含数据根，日志面统一由调用方决定；fail-soft 本身不发声）。
pub fn load_memory_section(
    data_root_mode: &Path,
    cwd: &str,
    memory_enabled: bool,
) -> Option<MemorySection> {
    MemoryContext::load(data_root_mode, cwd, memory_enabled)
        .map(|context| context.to_memory_section(&today_utc()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agent-server-memory-unit-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        path
    }

    /// `MODE_DATA_BASE_DIR` 下的 `.mode` 数据根（`load` 的入参形态）。
    fn mode_root(base: &Path) -> PathBuf {
        base.join(".mode")
    }

    fn seed_memory(base: &Path, slug: &str, hash: &str, index: &str) -> PathBuf {
        let dir = mode_root(base)
            .join("cli")
            .join("memories")
            .join("projects")
            .join(format!("{slug}-{hash}"))
            .join("memory");
        std::fs::create_dir_all(&dir).expect("create memory dir");
        std::fs::write(dir.join("MEMORY.md"), index).expect("write MEMORY.md");
        dir
    }

    #[test]
    fn disabled_does_not_touch_the_file_system() {
        // 关闭时连 projects 目录都不存在也不报错（零读取口径）。
        let base = temp_root("disabled");
        let loaded = MemoryContext::load(&mode_root(&base), "D:\\work\\ws", false);
        assert!(loaded.is_none(), "关闭时不注入");
    }

    #[test]
    fn missing_projects_root_is_fail_soft() {
        let base = temp_root("missing-root");
        assert!(
            MemoryContext::load(&mode_root(&base), "D:\\work\\ws", true).is_none(),
            "projects 目录缺失 = 无记忆"
        );
    }

    #[test]
    fn loads_index_by_slug_scan() {
        let base = temp_root("hit");
        let dir = seed_memory(
            &base,
            "ws",
            "0123456789abcdef",
            "# Project memory index\n\n- item-one: remember the sentinel ALPHA-123\n",
        );
        let loaded = MemoryContext::load(&mode_root(&base), "D:\\work\\ws", true).expect("命中");
        assert_eq!(loaded.workspace_id(), "ws-0123456789abcdef");
        assert_eq!(loaded.memory_dir(), dir);
        assert!(loaded.system_section().starts_with("# Memory\n\n"));
        assert!(loaded
            .system_section()
            .contains(&format!("`{}/`", dir.to_string_lossy())));
        let block = loaded.agents_md_block("2026-11-17");
        assert!(block.contains("Contents of "), "agentsMd 标题");
        assert!(block.contains("ALPHA-123"), "索引内容进入块");
        assert!(block.contains("Today's date is 2026-11-17."), "日期注入");
        assert!(!block.contains("BLUE-LANTERN-9"), "item 文件不读");
    }

    #[test]
    fn ambiguous_slug_is_fail_soft() {
        let base = temp_root("ambiguous");
        seed_memory(&base, "ws", "0123456789abcdef", "# one\n");
        seed_memory(&base, "ws", "fedcba9876543210", "# two\n");
        assert!(MemoryContext::load(&mode_root(&base), "D:\\work\\ws", true).is_none());
    }

    #[test]
    fn oversized_index_is_fail_soft() {
        let base = temp_root("oversized");
        let oversized = format!("# index\n{}", "x".repeat(5 * 1024 * 1024 + 1));
        seed_memory(&base, "ws", "0123456789abcdef", &oversized);
        assert!(MemoryContext::load(&mode_root(&base), "D:\\work\\ws", true).is_none());
    }

    /// 夹具逐字对照（数据契约）：模板重建的 `# Memory` 段与 agentsMd 块必须与
    /// `memory-recall.json` 的录制文本一致（只替换路径折叠位与日期）。
    ///
    /// 夹具在 Windows 上录制（路径分隔符 `\`），而本测试在临时目录构造真实路径；
    /// 两侧都把 `\` 归一成 `/` 后比较——内容结构逐字可比，分隔符差异不误报。
    #[test]
    fn rebuilt_texts_match_the_recorded_fixture() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("packages")
            .join("desktop")
            .join("tests")
            .join("agent-runtime-oracle")
            .join("fixtures")
            .join("c2")
            .join("memory-recall.json");
        let raw = std::fs::read_to_string(&fixture_path).expect("读取 memory-recall 夹具");
        let fixture: serde_json::Value = serde_json::from_str(&raw).expect("夹具是合法 JSON");
        let normalize = |text: &str| text.replace('\\', "/");

        let base = temp_root("fixture");
        let index_content = fixture["input"]["memoryFiles"]["MEMORY.md"]
            .as_str()
            .expect("夹具记录 MEMORY.md 内容");
        seed_memory(&base, "ws", "0123456789abcdef", index_content);
        let loaded = MemoryContext::load(&mode_root(&base), "D:\\work\\ws", true).expect("命中");

        // `# Memory` 系统段：折叠位 `<data-root>` → 数据基目录、`<workspace-id>` → id。
        let recorded_section = fixture["expect"]["withMemory"]["requests"][0]["memorySection"]
            .as_str()
            .expect("夹具录制 # Memory 段");
        let expected_section = recorded_section
            .replace("<data-root>", &base.to_string_lossy())
            .replace("<workspace-id>", loaded.workspace_id());
        assert_eq!(
            normalize(&loaded.system_section()),
            normalize(&expected_section),
            "# Memory 段必须与夹具录制逐字一致"
        );

        // agentsMd 块：索引路径折叠位 → 真实路径、`<date>` → 本测试的固定日期。
        let recorded_block = fixture["expect"]["withMemory"]["requests"][0]["memoryFileBlock"]
            .as_str()
            .expect("夹具录制 memoryFileBlock");
        let index_path = loaded
            .memory_dir()
            .join(project_memory::PROJECT_MEMORY_INDEX_FILE_NAME);
        let expected_block = recorded_block
            .replace("<data-root>", &base.to_string_lossy())
            .replace("<workspace-id>", loaded.workspace_id())
            .replace("<date>", "2026-11-17");
        let actual_block = loaded.agents_md_block("2026-11-17");
        assert!(
            actual_block.contains(&index_path.to_string_lossy().to_string()),
            "块内标题使用真实索引路径"
        );
        assert_eq!(
            normalize(&actual_block),
            normalize(&expected_block),
            "agentsMd 块必须与夹具录制逐字一致"
        );

        // 装配结果：系统段进 MemorySection.content，块进 user_prefix（T1 槽位）。
        let section = loaded.to_memory_section("2026-11-17");
        assert_eq!(section.content(), loaded.system_section());
        assert_eq!(section.user_prefix(), loaded.agents_md_block("2026-11-17"));
    }
}
