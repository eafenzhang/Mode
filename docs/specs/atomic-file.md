# Spec：原子写与跨进程文件锁（`crates/atomic-file`）

状态：2026-10-29 实现（W1 第一个基元）。行为真源：
`packages/services/src/fs/atomicFileUtils.ts`、`packages/shared/src/node/atomicFileLock.ts`。

## 为什么需要它

设置、凭据、任务 JSON 都是「读 → 改 → 写」。两个写者同时保存时，直接 `writeFile` 会把
文件写成半截；Windows 上 Defender/索引器还会短暂占用目标文件，让 `rename` 抛
`EPERM/EBUSY/EACCES`。本 crate 把这些行为固定下来，后续 settings / credential /
task-index 都复用它，而不是各写一份。

## 写入口

```rust
write_text_atomic(path, text, &options)
write_bytes_atomic(path, bytes, &options)
write_json_atomic(path, &serde_json::Value, &options)
```

顺序（失败任一步都要清理本次临时文件）：

1. `create_dir_all(parent)`；
2. 取文件锁（`use_file_lock=false` 可跳过，仅测试/单进程场景）；
3. 清理**同名目标**的过期临时文件（默认 60 s；当前临时文件除外）；
4. 写临时文件 `<basename>.<pid>.<millis>.<nonce>.tmp`（同目录，避免跨卷 rename）；
5. `rename` 到目标；只对 `EPERM/EBUSY/EACCES` 按 `[50,100,200,400,800,1600,3200] ms`
   退避重试，其它错误原样抛出。

JSON 输出与 TS `JSON.stringify(data, null, 2)` 同形：两空格缩进、**无尾随换行**。

## 锁布局

- 锁目录：`<目标文件>.lock`，用 `create_dir` 的原子性做 CAS。
- 拿到目录后写 `owner-<pid>-<millis>-<nonce>.json`（`pid`/`createdAt`/`token`），
  再读目录：**只有自己的 owner 文件**才算拿到；否则视为竞争，清掉自己的 owner 后重试。
- 释放：删自己的 owner，再 `remove_dir`。目录里出现别人的条目时 `remove_dir` 失败——
  这正是「不误删后来写者」的保护。
- 竞争退避 `[25,50,100,200,400] ms`，上限 8 s；**到上限直接超时，不在超时点回收**。

## 废弃锁回收

只在「等锁未超时」时尝试，且必须命中下列之一：

| 现场 | 条件 |
| --- | --- |
| 目录里恰好 1 个 owner | 该 owner 的 `pid` 已退出；或没有 `pid` 且 owner 文件年龄 ≥ grace（默认 100 ms） |
| 空目录 / 多个 owner | 目录本身年龄 ≥ grace，且每个 owner 都满足上一条 |
| 升级前的单文件锁 | 内容可回收，且删除前二次读取内容未变（防 ABA） |

`pid` 存活检测：Windows 用 `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` +
`GetExitCodeProcess == STILL_ACTIVE`；Linux 看 `/proc/<pid>`；其余 Unix 用 `kill -0`；
拿不准时**按存活处理**（宁可不回收）。

## 超时与错误

`AtomicError::LockTimeout { lock_path, waited_ms }`，文案与 TS 一致：
`Timed out after <n>ms waiting for the Mode file lock: <path>`。
IO 错误原样包装为 `AtomicError::Io`。

## 与 TS 实现的两处有意差异

1. 未实现 `lockInstanceObserver`（TS 用来防止「owner 文件 mtime 被刷新导致锁永不过期」）。
   这里以 owner 文件 mtime 作为 `createdAt` 兜底；锁持有者不会主动 touch 它。
2. 未校验锁目录的 dev/inode（Rust std 跨平台拿不到稳定 inode）。改为写入 owner 后校验
   目录里只有本写者条目：目录被替换时会看到多出来的 owner，按竞争处理。

## 验收

`crates/atomic-file/tests/atomic_write.rs`（8 条）：原子写与建父目录、6 线程并发写不产生
半截、过期临时文件清理、活进程持锁超时且不回收、死进程锁回收、空锁目录回收、JSON 形态、
显式取锁/释放。`cargo clippy -D warnings` 通过。
