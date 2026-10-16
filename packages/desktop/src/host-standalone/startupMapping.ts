/**
 * 运行时的 storage-startup 事实 → 渲染层要的 `DatabaseStartupState`（M3 Task 2 · DR-T2-2）。
 *
 * ## 为什么需要一层**显式翻译**，而不是把两边的 phase 名凑成同名
 *
 * 两个词汇表看起来像，其实不是一回事：
 *
 * | 来源 | phase 取值 | 说的是 |
 * | --- | --- | --- |
 * | 运行时 `startup/storageState` 帧（`packages/shared/src/mode-protocol`） | `checking` / `waiting_for_lock` / `migrating` / `committing` / `ready` / `failed` | **一个数据库**此刻在干什么 |
 * | 渲染层 `DatabaseStartupState`（`packages/shared/src/database-startup.ts`） | `starting` / `preparing_host_storage` / `preparing_session_storage` / `starting_services` / `ready` / `failed` | **整个应用启动**走到哪一步 |
 *
 * 两者的抽象层级不同：一个数据库 `ready` **不等于**应用 `ready`（还有另一个数据库与服务面），
 * 所以映射规则是显式的两条轴，而不是同名字段直通：
 *
 * - **应用阶段**由 `databaseKind` 决定（`tasks-index` → `preparing_host_storage`，
 *   `session` → `preparing_session_storage`；i18n 文案分别是「准备任务索引 / 准备聊天记录」）；
 * - **库内阶段**原样进 `databasePhase`（词汇表的公共部分是 `checking`/`waiting_for_lock`/
 *   `migrating`/`committing`/`ready`，两边的声明逐字一致）；
 * - 运行时的 `ready` 只把 `databasePhase` 推进到 `ready`，**不**把应用阶段推成 `ready` ——
 *   应用 `ready` 由 Host 自己（服务面装配完成）说了算；
 * - 运行时的 `failed` 带 `errorCode`（帧 schema 的 `superRefine` 强制），映射成应用 `failed`
 *   + `failedPhase`（失败时停在哪个应用阶段）+ 同码；
 * - `migration`（`DatabaseMigrationFacts`）两边是**同一个 schema**，原样带过去；
 *   正在迁移/提交时另填 `currentMigration`。
 *
 * ## 输入的真实性（登记）
 *
 * 计划里写「复用 M2 已录制的 `startup/storageState` 帧」，但**本 checkout 里没有那份录制**：
 * `packages/shared/test/fixtures/agent-protocol/` 下没有 `startup/storageState` 帧，
 * `packages/desktop/tests/agent-runtime-oracle/fixtures/` 里也没有（grep 零命中；那份夹具录的是
 * 工具调用序列）。所以本模块的夹具是**按声明 schema 构造**并当场用
 * `modeStorageStartupStateSchema.parse` 验过的——它证明的是「翻译对声明的协议形状成立」，
 * 不是「对某台机器某次真实运行录下来的字节成立」。差额登记在 `docs/specs/tauri-renderer-host.md`。
 *
 * ## `sequence` 的语义
 *
 * 状态自身的 `sequence` 是**本机发布计数器**（单调递增、从 0 开始），与壳的
 * `StartupMonitor` 同义（渲染层按它丢弃旧状态）；运行时帧里那个 `sequence` 是**单库流**的
 * 序号，两者不是一回事，因此不直通（直通会在两个库交替上报时倒退，而 `.strict()` 的 schema
 * 之外还有「旧状态不得覆盖新状态」的消费语义）。
 */
import {
  classifyDatabaseStartupError,
  databaseStartupErrorDetails,
  databaseStartupStateSchema,
  type DatabaseMigrationFacts,
  type DatabaseStartupErrorCode,
  type DatabaseStartupState,
  type ModeStorageStartupState,
} from "@mode/shared";

/** 应用阶段名（取值就是 `databaseStartupPhaseSchema` 的集合，不另立）。 */
export type StartupPhaseName = DatabaseStartupState["phase"];

/** 库内阶段名（`DatabaseStartupState["databasePhase"]` 的非可选形态）。 */
export type DatabasePhaseName = NonNullable<DatabaseStartupState["databasePhase"]>;

/**
 * 「一个数据库此刻在干什么」的**事实相位**：真存储回调的全集。
 *
 * 比 wire 帧的相位多一个 `maintaining`（`modeStorageStartupStateSchema` 的 `phase` 枚举没有它，
 * 真存储的 `TasksStoragePhase` 有）——所以事实不能用帧的类型来表达，否则生产路径那一步会被
 * 类型系统逼着丢掉 `maintaining`（丢掉就是渲染层看不到「迁移后修复中」）。
 */
export type DatabasePhaseFact =
  | "checking"
  | "waiting_for_lock"
  | "migrating"
  | "maintaining"
  | "committing"
  | "ready";

/**
 * 一条**库内事实**：真存储回调与 wire 帧在这一点上同形（`databaseId` / `databaseKind` /
 * 库内相位 / 迁移事实 / 失败细节）。
 *
 * 这个类型是「翻译」的输入——生产路径（真存储回调）与夹具路径（`applyStorageFrame`）
 * 都构造它，然后走**同一个** `StartupStateMachine.applyStorageFact`。
 */
export interface StoragePhaseFact {
  databaseId?: string;
  databaseKind: ModeStorageStartupState["databaseKind"];
  /** 库内相位；`failed` 表示这个库失败了（翻译成应用 `failed`，不进 `databasePhase`）。 */
  phase: DatabasePhaseFact | "failed";
  migration?: DatabaseMigrationFacts;
  errorCode?: DatabaseStartupErrorCode;
  sqliteCode?: number;
  systemCode?: string;
  migrationId?: string;
}

/**
 * wire 帧 → 事实。**唯一**需要知道帧形状的地方（真存储回调直接构造事实，不经这里）。
 */
export function storageFactFromFrame(frame: ModeStorageStartupState): StoragePhaseFact {
  return {
    ...(frame.databaseId !== undefined ? { databaseId: frame.databaseId } : {}),
    databaseKind: frame.databaseKind,
    phase: frame.phase,
    ...(frame.migration ? { migration: frame.migration } : {}),
    ...(frame.errorCode !== undefined ? { errorCode: frame.errorCode } : {}),
    ...(frame.sqliteCode !== undefined ? { sqliteCode: frame.sqliteCode } : {}),
    ...(frame.systemCode !== undefined ? { systemCode: frame.systemCode } : {}),
    ...(frame.migrationId !== undefined ? { migrationId: frame.migrationId } : {}),
  };
}

/** `databaseKind` → 应用阶段：这是「显式翻译」的核心一条。 */
export function applicationPhaseForDatabaseKind(
  databaseKind: ModeStorageStartupState["databaseKind"],
): StartupPhaseName {
  switch (databaseKind) {
    case "tasks-index":
      // 任务索引是 Host 自己的存储（i18n：startup.global.preparing_host_storage = "Preparing task index"）。
      return "preparing_host_storage";
    case "session":
      // 会话库是每个工作区的聊天记录（i18n：startup.global.preparing_session_storage = "Preparing chat history"）。
      return "preparing_session_storage";
    default: {
      // `databaseKind` 是 `.strict()` 的两值枚举；未来加值时必须在这里做一次显式决定，
      // 不能静默落进某个分支（静默落错会让渲染层的进度文案指错东西）。
      const exhaustive: never = databaseKind;
      throw new Error(`未映射的 databaseKind：${String(exhaustive)}`);
    }
  }
}

/** 库内相位 → 状态里的 `databasePhase`（`failed` 不进这一栏，它走应用 `failed`）。 */
function databasePhaseForFactPhase(phase: DatabasePhaseFact | "failed"): DatabasePhaseName | undefined {
  return phase === "failed" ? undefined : phase;
}

export interface StartupReportDetail {
  databaseId?: string;
  databaseKind?: ModeStorageStartupState["databaseKind"];
  databasePhase?: DatabasePhaseName;
  migration?: DatabaseMigrationFacts;
  errorCode?: DatabaseStartupErrorCode;
  /** 失败时停在哪个应用阶段（不给则取当前阶段）。 */
  failedPhase?: StartupPhaseName;
  sqliteCode?: number;
  systemCode?: string;
  migrationId?: string;
}

export interface StartupStateMachineOptions {
  startupId: string;
  attemptId: string;
  /** 时钟注入（用例要确定性）。 */
  now?: () => number;
}

/**
 * `DatabaseStartupState` 的唯一生产者（Host 侧）：所有输出都过一次
 * `databaseStartupStateSchema.parse`，所以在某个字段写错的**那一刻**就红，
 * 而不是等到渲染层因为 `.strict()` 把整条消息丢掉（症状是「闸门不放行、也没有失败原因」）。
 */
export class StartupStateMachine {
  #startedAt: number;
  #now: () => number;
  #state: DatabaseStartupState;
  #sequence = 0;

  constructor(options: StartupStateMachineOptions) {
    this.#now = options.now ?? (() => Date.now());
    this.#startedAt = this.#now();
    this.#state = this.#validate({
      schemaVersion: 1,
      startupId: options.startupId,
      attemptId: options.attemptId,
      sequence: 0,
      startedAt: this.#startedAt,
      updatedAt: this.#startedAt,
      phase: "starting",
      disk: [],
    });
  }

  #validate(candidate: unknown): DatabaseStartupState {
    const parsed = databaseStartupStateSchema.safeParse(candidate);
    if (!parsed.success) {
      throw new Error(
        `Host 生成的 DatabaseStartupState 不符合 schema（.strict()）：${parsed.error.message}`,
      );
    }
    return parsed.data;
  }

  /** 推进到一个应用阶段（迁移事实/错误码按需带上；未给的字段不出现）。 */
  report(phase: StartupPhaseName, detail: StartupReportDetail = {}): DatabaseStartupState {
    this.#sequence += 1;
    const next: DatabaseStartupState = {
      schemaVersion: 1,
      startupId: this.#state.startupId,
      attemptId: this.#state.attemptId,
      sequence: this.#sequence,
      startedAt: this.#startedAt,
      updatedAt: this.#now(),
      phase,
      disk: [],
    };
    if (phase === "failed") {
      // 失败必须能说出「停在哪个应用阶段」与一个错码（渲染层的重试判据读这两个字段）。
      next.failedPhase =
        detail.failedPhase ?? (this.#state.phase === "failed" ? this.#state.failedPhase : this.#state.phase) ?? "starting";
      next.errorCode = detail.errorCode ?? "sql_failed";
    } else if (detail.errorCode) {
      next.errorCode = detail.errorCode;
    }
    if (detail.databasePhase) next.databasePhase = detail.databasePhase;
    if (detail.migration) {
      next.migration = detail.migration;
      if (detail.databasePhase === "migrating" || detail.databasePhase === "committing") {
        next.currentMigration = detail.migration;
      }
      if (detail.databaseId && detail.databaseKind) {
        next.migrationBaselines = [
          {
            databaseId: detail.databaseId,
            databaseKind: detail.databaseKind,
            ...(detail.migration.lastAppliedMigrationId !== undefined
              ? { lastAppliedMigrationId: detail.migration.lastAppliedMigrationId }
              : {}),
          },
        ];
      }
    }
    if (detail.sqliteCode !== undefined) next.sqliteCode = detail.sqliteCode;
    if (detail.systemCode !== undefined) next.systemCode = detail.systemCode;
    if (detail.migrationId !== undefined) next.migrationId = detail.migrationId;
    this.#state = this.#validate(next);
    return this.#state;
  }

  /**
   * 由异常推进到 `failed`：错误码用仓库既有的 `classifyDatabaseStartupError`（不自己编分类），
   * 细节取 `databaseStartupErrorDetails`（只带码与迁移 id，不带 SQL/文件内容/凭据）。
   */
  fail(
    error: unknown,
    options: { phase?: StartupPhaseName; errorCode?: DatabaseStartupErrorCode } = {},
  ): DatabaseStartupState {
    const details = databaseStartupErrorDetails(error);
    return this.report(options.phase ?? "failed", {
      errorCode: options.errorCode ?? classifyDatabaseStartupError(error),
      ...details,
    });
  }

  /**
   * **唯一的翻译实现**：库内事实 → 应用阶段（DR-T2-2 的「显式翻译」就在这里）。
   *
   * **生产路径与夹具路径都走这一个函数**——真存储回调（`index.ts` 的 `preparing_host_storage`
   * 那一段）直接调它，夹具经 `applyStorageFrame` 调它。这条纪律是本任务复评的 I1：
   * 复评前的形态是「映射写了两遍」（入口把应用阶段硬编码成字面量），于是**夹具断言的是死代码**，
   * 出货的那条路无人断言——出问题只会表现为「渲染层的进度文案指错东西」，最难查。
   * 现在有一条源码扫描用例钉住「入口不得硬编码应用阶段、必须调本函数」。
   */
  applyStorageFact(fact: StoragePhaseFact): DatabaseStartupState {
    const applicationPhase = applicationPhaseForDatabaseKind(fact.databaseKind);
    if (fact.phase === "failed") {
      return this.report("failed", {
        // 失败帧说得出「哪个库」→ 应用阶段（`failedPhase`），那正是重试/退出判据要读的字段。
        failedPhase: applicationPhase,
        ...(fact.databaseId !== undefined ? { databaseId: fact.databaseId } : {}),
        databaseKind: fact.databaseKind,
        errorCode: fact.errorCode ?? "sql_failed",
        ...(fact.sqliteCode !== undefined ? { sqliteCode: fact.sqliteCode } : {}),
        ...(fact.systemCode !== undefined ? { systemCode: fact.systemCode } : {}),
        ...(fact.migrationId !== undefined ? { migrationId: fact.migrationId } : {}),
        ...(fact.migration ? { migration: fact.migration } : {}),
      });
    }
    const databasePhase = databasePhaseForFactPhase(fact.phase);
    const detail: StartupReportDetail = {
      ...(fact.databaseId !== undefined ? { databaseId: fact.databaseId } : {}),
      databaseKind: fact.databaseKind,
      ...(databasePhase ? { databasePhase } : {}),
      ...(fact.migration ? { migration: fact.migration } : {}),
    };
    return this.report(applicationPhase, detail);
  }

  /**
   * **夹具驱动的翻译**：把一帧真实形状的运行时 `startup/storageState` 应用到状态机上
   * （DR-T2-2 要求的输入形状；也是 Task 3/4 那条 Host → 壳 通道落地后要调的入口）。
   *
   * 帧本身先过一次 `modeStorageStartupStateSchema` 是**调用方**的事（用例里做了），
   * 这里只负责「帧 → 事实」的形状转换，翻译逻辑在 `applyStorageFact` 里（同一份）。
   */
  applyStorageFrame(frame: ModeStorageStartupState): DatabaseStartupState {
    return this.applyStorageFact(storageFactFromFrame(frame));
  }

  snapshot(): DatabaseStartupState {
    return this.#state;
  }
}
