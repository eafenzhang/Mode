# Spec：设备身份 deviceMid（`crates/device-mid`）

状态：2026-10-29 实现（W1 基元）。行为真源：`packages/services/src/device/deviceMid.ts`。

## 为什么需要

`deviceMid` 是跨端共享的设备身份：计费 header `X-Device-Mid`、反馈、onboarding 记录、
对话镜像都读它。CLI、Desktop、远端 server 读写**同一个文件、同一个字段**，所以文件名与
字段名都不能改——改名等于重置用户的设备身份。

## 落点与格式

```
{dataRoot}/v2/telemetry-state.json
{ "deviceMid": "<uuid v4>", ...其它字段原样保留 }
```

`dataRoot` 由调用方传入（= `{dataBaseDir}/.mode`）。文件里的其它字段（如
`telemetryOptOut`）在写回时**必须保留**：我们读成 JSON 对象再插入 `deviceMid`。

## 行为

1. 读取文件：缺失、非对象、JSON 损坏、`deviceMid` 为空或非字符串 → 视为**没有身份**
   （TS 同样把坏文件当成「从未记录」，下次写入时重建）。
2. 已有合法 `deviceMid` → 直接返回。
3. 没有 → 取 `<文件>.lock` 锁 → **锁内重读**（防两个进程各生成一个 id）→ 仍没有则生成
   UUID v4 并原子写回 → 返回。
4. UUID v4 只要求格式合法（非加密用途）；熵来自 `RandomState`（进程级 OS 种子）+ 时间 +
   pid + 计数器。

## 与 TS 的差异

- **锁的实现不同**：TS 用 `<文件>.lock` 普通文件 + owner JSON；这里用 `crates/atomic-file`
  的目录锁（`<文件>.lock` 目录 + owner 文件）。两者互不兼容，但同一版本内不会混用；
  数据文件本身跨端一致。
- **写回是原子的**（临时文件 + rename），TS 是直接 `writeFile`。
- **不做进程内缓存**：TS 用 `Map<file, Promise>` 缓存；Rust 侧调用方需要时自行缓存，
  重复调用只是一次读（命中即返回）。

## 验收

`crates/device-mid/tests/device_mid.rs`（4 条）：生成并复用、保留未知字段、坏文件重建、
4 线程并发得到同一 id。`cargo clippy -D warnings` 通过。
