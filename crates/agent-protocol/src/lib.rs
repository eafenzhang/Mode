//! `agent-protocol`：**旧 agent 运行时在 stdio 上实际说的话**的 Rust 镜像。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §1.2（stdio 的实测线上形状）。
//! 纪律：`docs/specs/clean-room-policy.md`——本 crate 全新编写，只取材于**允许输入**
//! （协议类型声明与帧结构、导出签名、测试断言）与**黑盒观察**（跑旧实现、看它的输入输出与
//! 事件序列），不读旧运行时的实现体。
//!
//! ## 镜像的是哪个协议面
//!
//! 镜像面是 **stdio 会话面**：方法名信封 `{method, params}`（+ `{id, result}` /
//! `{id, error}`）与 `session/event` 上承载的会话事件。**不是** `mode-protocol-v4/**` 的
//! 主题帧——v4 是 host（web/远端）面，stdio 运行时不说它。
//!
//! 依据（两条独立观察一致）：运行时 stdout 上是 `{"method":"startup/storageState",…}` 这类
//! 方法名帧；会话面是 `session/requestRuntimePreferences` / `session/create` /
//! `session/subscribe` / `session/send` + `session/event` 通知。声明面也落在
//! `packages/shared/src/mode-protocol/index.ts`（v1，方法表与 `modeProtocolMessageSchema`）
//! 与 `apps/mode-cli/packages/contracts/src/events/session.events.ts`（接口面），而不是 v4。
//!
//! ## 本次建模范围（YAGNI）
//!
//! 只落 M2 对拍与最小会话循环要用到的面：帧形状、会话事件信封与只读切片实际产生的事件载荷、
//! 以及 host 命令面里 M2 会发/会答的那几条（`session/create`、`session/send`、
//! `session/requestRuntimePreferences`）。`session_read` 是 W4 会话只读组的第一片：
//! `session/list` 的严格镜像，依据协议声明 schema，并用双端夹具锁住形状
//! （见 `docs/specs/session-read-group.md`）。v1 方法表与 v4 各有几十个类型，一次翻译整套既是
//! 无用代码，也接近 clean-room 禁止的「按上游思路复现」。后续任务按需补，补的时候同步补一条
//! **录制的**夹具（见下）。
//!
//! ## 严格度规则（按线上契约是否封闭决定）
//!
//! - **帧形状与事件信封是封闭契约** → `deny_unknown_fields`：多一个键就整体拒绝，与 TS 侧
//!   zod `modeProtocolMessageSchema` / `modeEventEnvelopeSchema` 的 `.strict()` 同口径。
//!   漂移必须**响**，不能静默吞掉。
//! - **判别键**（事件 `type`、`tool.updated` 的 `kind`、`model.streaming` 的 `kind`）→ 取值
//!   写成字面量白名单；未知取值报错。比较器按它们分类与排序，静默接受未知取值会把
//!   「未覆盖」伪装成「已覆盖」。字面量逐条写死而不用 `rename_all` 推导：M1 踩过
//!   `OAuthCallback` → `o-auth-callback`（见 `crates/protocol/src/lib.rs` 的注释）。
//! - **载荷内部是会长的部分** → 已知字段声明为类型，其余落进 `extra`
//!   （`#[serde(flatten)]`）。这样运行时多一个可选字段不会让整条事件/命令解析失败，而镜像
//!   取到的字段**类型不符仍然立刻报错**。协议里本就声明为开放对象的（`session.updated` 的
//!   载荷、`tool.updated.result.result`、`model.streaming.input`）只建模要点、其余原样保留。
//!
//! ## 夹具与两端互证
//!
//! `packages/shared/test/fixtures/agent-protocol/*.json` 是**两端共读**的夹具：TS 侧用 v1
//! zod schema 校验（`packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts`），
//! Rust 侧用本 crate 的镜像类型校验（`crates/agent-protocol/tests/fixtures.rs`）。夹具由
//! `packages/desktop/tests/agent-runtime-oracle/record-protocol-fixtures.mts` 从 oracle 的
//! stdout/stdin 上**整帧原样**录下——不是照着镜像类型写的。两端各自用自己的类型解同一份字节，
//! 所以哪一端单方面改类型而另一端没跟上，那一端就会红。

pub mod frame;
pub mod message;
pub mod permission;
pub mod session;
pub mod session_debug;
pub mod session_event;
pub mod session_read;
pub mod snapshot;

pub use frame::{
    ErrorObject, ErrorResponseFrame, ExtraFields, Frame, NotificationFrame, RequestFrame,
    RequestId, ResponseFrame, ReverseRequestIds,
};
pub use permission::{
    PermissionDecision, PermissionFrameError, PermissionOption, PermissionOptionKind,
    PermissionRequest, PermissionResponse, PermissionRuleValue, PermissionUpdate,
    PermissionUpdateType, RuleBehavior, REQUEST_PERMISSION_METHOD,
};
