# Spec：Onboarding 完成记录服务（Rust `onboarding-record-service`）

状态：2026-11-01 首版（9/9 方法，TS 对拍全绿；频道未注册）。
真源：`packages/services/src/onboarding/onboardingRecordService.ts`、
`packages/shared/src/onboardingRecord.ts`。

## 1. 范围

| Rust 方法                          | TS 方法                     | 说明                                         |
| ---------------------------------- | --------------------------- | -------------------------------------------- |
| `append_record`                    | `appendRecord`              | 同 userId（含 null）覆盖；清掉该用户判定    |
| `claim_anonymous_record`           | `claimAnonymousRecord`      | 把匿名条目/判定改写为当前 userId             |
| `should_onboard`                   | `shouldOnboard`             | 已有记录 → false；本机有任务 → 落判定并 false |
| `dismiss_onboarding`               | `dismissOnboarding`         | 用户关闭引导；已有作答时空操作               |
| `get_latest_entry`                 | `getLatestEntry`            | 当前用户最近一条作答                         |
| `sync_settings_from_record`        | `syncSettingsFromRecord`    | 回填 settings 的保守补丁                     |
| `update_record_preferences`        | `updateRecordPreferences`   | 手动改偏好后反向回写 record                  |
| `get_records`                      | `getRecords`                | 整份记录（后续上传服务器用）                 |
| `clear_records`                    | `clearRecords`              | 删除记录文件（调试用）                       |

依赖注入：`OnboardingRecordEnvironment::load_user_id` / `has_existing_local_task` 对应
TS 构造参数；返回 `Err` 等价于 rejected promise。时钟以 `now_ms` 注入，`decidedAt`
用 `iso8601_utc`（`Date.prototype.toISOString` 等价）生成。

## 2. 存储与格式

- 文件：`<record_path>`（宿主传 `<appConfigDir>/onboarding-record.json`），设备级数据；
- 顶层：`version`（1/2）、`deviceMid`、`entries`、`decisions`；
- v1 读取时补空 `decisions` 并升级为 v2；写回始终 v2；
- 写入使用 `atomic-file`（临时文件 + rename + 跨进程锁），两空格缩进、无尾随换行，
  与 `JSON.stringify(file, null, 2)` 同形。

## 3. 校验与损坏容忍

- 读取：任何 IO 失败、非 JSON、schema 不匹配都按「从未记录」处理（`None`），不抛错；
- `entry` 要求 `userId`/`occupation`/`interfaceMode`/`memoryEnabled`/
  `proactiveSuggestionsEnabled`/`completedAt`/`uploadState` 七个键全部存在；
  `userId`、`occupation` 为 null 或非空字符串，`interfaceMode` 为 `coding|office|null`，
  `uploadState` 必须为 `pending`；未知键忽略（与 zod 非 strict 对象一致）；
- `decision` 的 `status` 为 `dismissed|existing_local_user`，`reason` 为
  `user_closed|existing_local_task`；
- `appendRecord` 的入参在写入前按同一 schema 形状校验。

## 4. 语义要点

- `deviceMid` 以文件内已有值为权威；传入不同值不改写旧值。
- `append_record` 覆盖而不是追加；同一 userId 的旧判定被清除（作答后不再算跳过）。
- `should_onboard` 的判定写入发生在「无记录 + 本机已有任务」时；已有记录/判定直接
  false，不写任何东西。
- `sync_settings_from_record`：未知/已收窄/为 null 的职业回填 `other`；两个偏好
  按 `?? false` 回填。
- 写操作串行化（进程内 `Mutex`），对应 TS 的 promise 写队列。

## 5. 与 TS 的差异

1. **同步 API**：Rust 服务方法是同步的；依赖回调由调用方保证不阻塞事件循环
   （频道适配时用 `spawn_blocking`）。TS 的 `await` 顺序语义不变。
2. **错误文案**：入参校验错误是 Rust 文案，不是 zod 的 issue 列表；正常路径与
   文件内容不受影响。
3. **频道未接管**：`onboarding-record` 仍由 Host 的 TS 服务提供；本次只迁服务层。
   onboarding 流程本身仍受 OAuth / 任务库依赖约束。

## 6. 验收

- `pnpm onboarding-record:fixtures` → `crates/onboarding-record-service/fixtures/records.json`
  （7 条用例：覆盖写、匿名认领、shouldOnboard 判定、dismiss、settings 回填、v1 升级、
  损坏文件与清理；`decidedAt` 归一化为 `<ts>`，`rawFile` 校验落盘文本）。
- `cargo test -p onboarding-record-service`（含 ISO 格式化单测）。
- `cargo clippy -p onboarding-record-service --all-targets -- -D warnings`。
