# Spec：CUA raw input socket 解析策略（Rust）

> 真源：`packages/services/src/cua-permission-broker/cuaRawInput.ts` 的
> `createCuaRawInputSocketResolver`。本片只迁 socket 解析策略；
> broker 调用、`createCuaRawInputInjector`、错误码收敛与真实 socket 发现留在宿主。

## 1. 范围与所有者

| Rust 入口 | TS 对应 |
| --- | --- |
| `RawInputSocketResolverOptions` | `CreateCuaRawInputSocketResolverOptions` |
| `RawInputSocketResolver::resolve` | 返回的 resolver 闭包 |

- **唯一策略点**：live peek → 稳定 socket 探测 → 被控开关打开时触发创建；
  前两支只发现、不拉起，开关关闭时无创建副作用。
- 创建过程完全归注入的 `ensure_remote_desktop_helper`：本层不 fork、不写 helper 状态、
  不重试掩盖。
- 并发：Rust 用 `&mut self` 串行化；宿主可用锁包一层。TS 的 async in-flight promise
  共享由宿主编排层负责。

## 2. 解析顺序

```text
resolve():
  live peek 命中              → Some(path)
  stable probe 命中           → Some(path)
  ensure 未注入               → None
  被控开关关闭                → None
  now < deferredUntil         → None（失败退避窗口）
  ensure() 成功               → Some(path)
  ensure() 失败               → None，deferredUntil = now + backoff
```

- `ensure_failure_backoff_ms = 0` 表示每次调用都重新尝试。
- 退避只在创建失败时写入；成功不重置窗口（窗口已过期，等价于无退避）。
- 创建入口抛错在宿主层收敛为 `None`；本层只接受 `Option<String>`。

## 3. 失败语义

| 场景 | 行为 |
| --- | --- |
| helper 不可达且开关关闭 | `None`，无任何创建调用 |
| ensure 未接线 | `None`，probe-only |
| 创建失败 | `None`，写入退避窗口 |
| 退避窗口内 | `None`，不调用创建 |
| live/stable 命中 | 直接返回，不触发创建 |

## 4. 与 TS 的已知差异

| 差异 | 说明 |
| --- | --- |
| 同步 API | TS 返回 `() => Promise<string|null>`；Rust 为 `resolve(&mut self)`。 |
| 无 in-flight 去重 | TS 并发帧共享同一 ensure promise；Rust 串行化，宿主负责共享。 |
| 无 AbortSignal/错误收敛 | broker 调用与 `CuaHelperError` 映射在本层之外。 |
| 时间注入 | `now_ms` 由调用方注入，便于退避测试。 |

## 5. 验收

- `cargo test -p cua-raw-input`：7 条用例覆盖 live 优先、stable 优先、开关关闭不创建、
  ensure 未接线、创建成功、失败退避与 0 退避重试。
- broker injector、socket 真实发现、PiP session、Windows helper host 未迁；
  Node 实现仍是 oracle。
