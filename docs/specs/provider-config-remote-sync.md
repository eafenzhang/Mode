# Spec：Mode Built-in Remote Release 刷新控制与编排（Rust）

> 真源：`packages/provider-node/src/mode-builtin-remote-synchronizer.ts` 的
> `RefreshControl` / `readControl` / `writeControl` 与 `#refresh` / `#finishLease`
> 状态转移。网络 fetch、`applyRemoteRelease`、AbortSignal 与 in-flight 去重仍在宿主编排层。

## 1. 范围

| Rust 入口 | TS 对应 | 说明 |
| --- | --- | --- |
| `RefreshControl` | `RefreshControl` | 控制文件模型（Endpoints 隔离） |
| `read_refresh_control` | `readControl` | 缺失/损坏/schema 非法 → 空控制 |
| `write_refresh_control` | `writeControl` | 私有原子写；调用方持锁 |
| `acquire_refresh_lease` | `#refresh` 的锁内段 | 租约/not-due/force/端点切换 |
| `finish_refresh_lease` | `#finishLease` | 成功间隔、失败退避、取消保留计数 |

控制文件路径由 `cache_paths` 的 `control_file_path` 提供；本模块不决定路径。

## 2. 控制文件

```json
{
  "schemaVersion": 1,
  "endpointKey": "https://example.com",
  "leaseId": "uuid",
  "leaseUntil": 1700000030000,
  "nextEligibleAt": 1700003600000,
  "failureCount": 0
}
```

- `schemaVersion` 必须为 1；缺失/损坏/字段类型不符时按空控制处理（不阻断刷新）。
- `leaseId` 可缺省（表示空闲）。
- `leaseUntil` / `nextEligibleAt` 为整数毫秒；`failureCount` 为非负整数。

## 3. 租约与退避

```text
acquire(endpoint, leaseId, force, now, leaseDuration):
  same = current.endpointKey == endpoint
  same && leaseUntil > now       → LeaseHeld
  !force && same && nextEligibleAt > now → NotDue
  写入 { endpoint, leaseId, leaseUntil: now+leaseDuration,
         nextEligibleAt: same ? current.nextEligibleAt : 0,
         failureCount: same ? current.failureCount : 0 }

finish(endpoint, leaseId, outcome, now):
  租约不匹配（endpoint 或 leaseId）→ 不写，返回 None
  success   → failureCount=0, next=now+successInterval
  failure   → failureCount+1, next=now+min(base*2^(count-1), max)
  cancelled → failureCount 不变, next=now
```

- 端点切换只重置 `nextEligibleAt` 与 `failureCount`，不读取上一 Endpoint 的缓存。
- 失败退避指数使用饱和运算，上限 `failureMaxDelayMs`；`failureBaseDelayMs` 至少按 1ms 处理。
- 所有读写都在控制文件跨进程锁内；首次写入会创建父目录。
- 控制文件不是业务事实：损坏内容在下次 acquire 时重建为空控制。

## 4. 失败语义

| 场景 | 行为 |
| --- | --- |
| endpointKey 为空/空白 | `Mode Built-in 远端 Endpoint 不能为空` |
| 活动中租约 | `LeaseHeld`，不写文件 |
| 未到 nextEligibleAt 且非 force | `NotDue`，不写文件 |
| 租约被他人替换后 finish | 返回 `None`，不覆盖他人状态 |
| 控制文件损坏/缺字段 | 按空控制继续 |
| 锁超时/写失败 | 错误上抛，交由宿主重试 |

## 5. 与 TS 的已知差异

| 差异 | 说明 |
| --- | --- |
| 同步 API | TS 为 async；Rust 为同步，异步边界由宿主包阻塞任务。 |
| 无 AbortSignal/in-flight 去重 | `refresh` 的并发共享与取消由宿主编排层负责；本模块只做锁内状态转移。 |
| leaseId 由调用方提供 | TS 内部 `randomUUID`；Rust 不引入 UUID，调用方传入。 |
| 时间与间隔显式传入 | 不读系统时钟；`now` 与三个间隔由调用方注入。 |

## 6. 验收

- `cargo test -p provider-config-repository --test remote_sync`：10 条用例覆盖首次租约、
  租约阻塞、成功窗口与 force、端点切换重置、失败退避翻倍、取消保留计数、租约不匹配、
  损坏控制文件、父目录创建与空 Endpoint。
- `cargo test -p provider-config-repository --test remote_refresh`：8 条用例覆盖租约跳过、
  not-due/force、三种 apply 结果、missing、端点切换、fetch/apply/端点解析失败后的失败结算。
- 真实 HTTP fetch/apply 与真机远端刷新未开始；Node 编排仍是 oracle。

## 7. 刷新编排（2026-11-03）

```rust
pub fn refresh_mode_builtin_release<F, R, A>(
    options: &RefreshOptions<'_>,
    fetch_release: F,          // FnOnce(&endpoint) -> Result<Option<ModeBuiltinRelease>, String>
    resolve_endpoint_key: R,   // FnOnce() -> Result<String, String>
    apply_release: A,          // FnOnce(ModeBuiltinRelease) -> Result<ApplyOutcome, String>
) -> Result<RefreshOutcome, String>;
```

顺序：

```text
acquire lease
  ├─ LeaseHeld / NotDue → Skipped(reason)，不 fetch、不 finish
  └─ Acquired
       ├─ fetch(endpoint) 失败 → finish Failure → Err
       ├─ 重新解析 endpoint 失败 → finish Failure → Err
       ├─ endpoint 变化 → finish Success → Skipped(EndpointChanged)
       ├─ release 缺失 → finish Success → Missing
       └─ apply
            ├─ 失败 → finish Failure → Err
            └─ Updated / Unchanged / Stale → finish Success → 对应结果 + revision
```

- `RefreshOutcome` 携带 `result`、可选 `reason` 与可选 `revision`；`Missing` 无 revision。
- 端点变化按 TS 口径结束为 **Success**（不是失败），避免把正常切换计入退避。
- 取消/AbortSignal、in-flight 去重与 disposed 结果由宿主编排层处理；本函数只执行一次
  同步状态转移，不读系统时钟。
- fetch/apply 由宿主注入；真实网络、`applyRemoteRelease` 的物化与 LKG 选择仍在后续切片。
