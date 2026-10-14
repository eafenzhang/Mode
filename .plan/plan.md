# 远程桌面光标可见性优化（已获用户批准执行）

依据：2026-10-13 实机诊断（被控机 DUPL 零指针数据 + 两处光标语义缺陷），执行顺序 = user32 光标兜底 → 两处 render_* 语义修复 → helper 日志接线。延迟问题不改代码（等状态条读数）。

## 1. spec 先行
`docs/specs/lan-remote-desktop-native-media.md`：
- 「光标」节：新增 user32 兜底（DUPL 从未投递指针数据 = 软件光标/虚拟显示环境时，经 GetCursorInfo/GetIconInfo/GetDIBits 采样喂现有 CursorShapeRaster 管线；DUPL 一旦投递形状即回归 DUPL 权威）。
- 「光标」节：修正 TYPE_COLOR（32bpp ARGB，alpha 即形状，不得强推不透明）与 TYPE_MASKED_COLOR（alpha 位即掩码 0=替换/0xFF=XOR，无尾随 1bpp 掩码）语义；含 XOR 像素的形状走 CPU 补丁路径（GPU 双流无法表达 XOR）。
- 「失败语义/可观测」：helper 侧 `[rd_media]` warn（GPU 回退、形状渲染失败等）接入主日志。

## 2. Rust：`crates/mode-cua-ax/src/rd_media/capture.rs`
- `render_color`：保留真实 alpha；`visible = alpha > 0`（全 0 alpha 按 ARGB 判空；无 alpha 的位图源由 user32 兜底用掩码处理）。
- `render_masked_color`：改为官方 32bpp ARGB 布局（仅校验 pitch*h），alpha==0xFF → XOR 标记，其余 → 替换；`CursorShapeRaster` 增加 `xor: Vec<u8>`。
- CPU 补丁混合因式拆成纯函数：alpha 混合 + XOR(bg ^ color)；GPU 覆盖层遇 XOR 形状整形状走 CPU 路径。
- user32 兜底：`dupl_shape_seen` 标记 + `sample_user32_pointer()`（GetCursorInfo 定位/可见性，GetIconInfo+GetDIBits 取位图；32bpp 真 alpha / 无 alpha 用 AND 掩码 / 单色走既有 render_monochrome 布局；句柄级缓存形状），合成前统一解析指针状态（DUPL 权威或 user32），`compute_cursor_rect`/`build_cursor_patch`/快照判重改用解析结果。

## 3. 测试
- 重写 `tests/rd_media.rs` 两处 render 语义断言（COLOR alpha 保留、MASKED 新布局/XOR 标记）；新增 CPU 混合（alpha+XOR）纯函数单测；更新 `capture.rs` 内 `CursorShapeRaster` 字面量（`xor` 字段）。
- Rust：`cargo test`（gnu 工具链本机环境）+ `cargo clippy -D warnings`；TS 不涉行为改动，跑 `pnpm typecheck` / `pnpm lint` 报告真实结果。

## 4. helper 日志接线
定位 cua helper 进程的 spawn 点，把其 stderr（`[rd_media]` warn）中继进主日志（warn 级）。

## 5. 交付
本地提交（不推送，除非用户要求）；两台实机回归项（光标随动、换屏、XOR 光标）留给用户。
