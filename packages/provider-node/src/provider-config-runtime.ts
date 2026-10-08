import {
  ProviderConfigService,
  type ProviderConfigLayerSnapshot,
  type ProviderConfigLayerUpdate,
} from "@mode/provider";
import { NodeModeBuiltinProviderConfigSource } from "./mode-builtin-provider-config-source.js";
import {
  EndpointScopedModeBuiltinSource,
  type EndpointScopedModeBuiltinSourceOptions,
} from "./endpoint-scoped-mode-builtin-source.js";
import {
  ModeBuiltinRemoteSynchronizer,
  type ModeBuiltinRemoteSynchronizerOptions,
  type ModeBuiltinRefreshResult,
} from "./mode-builtin-remote-synchronizer.js";
import {
  NodePersonalProviderConfigRepository,
  type PersonalProviderConfigRecoveryEvent,
} from "./personal-provider-config-repository.js";
import { createRetiredZhipuProviderMigrationUpdate } from "./retired-zhipu-provider-migration.js";

export interface NodeProviderConfigRuntimeOptions {
  readonly modeBuiltinFilePath: string;
  readonly modeBuiltinActiveFilePath?: string;
  readonly modeBuiltinRemote?: Omit<ModeBuiltinRemoteSynchronizerOptions, "source">;
  readonly modeBuiltinEnvironment?: Omit<
    EndpointScopedModeBuiltinSourceOptions,
    "bundledFilePath"
  >;
  readonly onModeBuiltinRefreshError?: (error: unknown) => void;
  readonly onPersonalConfigRecovery?: (event: PersonalProviderConfigRecoveryEvent) => void;
  readonly onPersonalConfigPollingError?: (error: unknown) => void;
  readonly personalFilePath: string;
  readonly personalPollingIntervalMs?: number | false;
  readonly onRetiredProviderMigrationError?: (error: unknown) => void;
  readonly importLegacy?: (
    modeBuiltin: ProviderConfigLayerSnapshot,
  ) => Promise<ProviderConfigLayerUpdate | null>;
  readonly watch?: boolean;
}

/** 组装一个 Node.js 进程内共享的 Mode Built-in/Personal Config 运行边界。 */
export class NodeProviderConfigRuntime {
  readonly configService: ProviderConfigService;
  readonly #modeBuiltinSource:
    | NodeModeBuiltinProviderConfigSource
    | EndpointScopedModeBuiltinSource;
  readonly #personalRepository: NodePersonalProviderConfigRepository;
  readonly #remoteSynchronizer?: ModeBuiltinRemoteSynchronizer;
  readonly #onRemoteRefreshError?: (error: unknown) => void;
  readonly #onRetiredProviderMigrationError?: (error: unknown) => void;
  #startPromise: Promise<void> | null = null;
  #disposed = false;
  readonly #checkListeners = new Set<() => Promise<void>>();
  #checkTimer: ReturnType<typeof setInterval> | null = null;
  #checkInFlight: Promise<void> | null = null;

  constructor(options: NodeProviderConfigRuntimeOptions) {
    this.#modeBuiltinSource = options.modeBuiltinEnvironment
      ? new EndpointScopedModeBuiltinSource({
          bundledFilePath: options.modeBuiltinFilePath,
          ...options.modeBuiltinEnvironment,
        })
      : new NodeModeBuiltinProviderConfigSource({
          bundledFilePath: options.modeBuiltinFilePath,
          activeFilePath: options.modeBuiltinActiveFilePath,
          watch: options.watch,
        });
    this.#remoteSynchronizer =
      options.modeBuiltinRemote &&
      this.#modeBuiltinSource instanceof NodeModeBuiltinProviderConfigSource
        ? new ModeBuiltinRemoteSynchronizer({
            source: this.#modeBuiltinSource,
            ...options.modeBuiltinRemote,
          })
        : undefined;
    this.#onRemoteRefreshError = options.onModeBuiltinRefreshError;
    this.#onRetiredProviderMigrationError = options.onRetiredProviderMigrationError;
    this.#personalRepository = new NodePersonalProviderConfigRepository({
      filePath: options.personalFilePath,
      onRecovery: options.onPersonalConfigRecovery,
      onPollingError: options.onPersonalConfigPollingError,
      pollingIntervalMs: options.personalPollingIntervalMs,
      ...(options.importLegacy
        ? {
            importLegacy: async () => options.importLegacy!(await this.#modeBuiltinSource.read()),
          }
        : {}),
    });
    this.configService = new ProviderConfigService({
      modeBuiltinSource: this.#modeBuiltinSource,
      personalRepository: this.#personalRepository,
    });
  }

  resolveModeBuiltinActiveFilePath(): Promise<string> {
    return this.#modeBuiltinSource instanceof NodeModeBuiltinProviderConfigSource
      ? Promise.resolve(this.#modeBuiltinSource.activeFilePath)
      : this.#modeBuiltinSource.resolveActiveFilePath();
  }

  get personalRepository(): import("@mode/provider").PersonalProviderConfigRepository {
    return this.#personalRepository;
  }

  /** Environment 同一周期检查中恢复未对齐依赖，不被下载 TTL 或失败挡住。 */
  onDidCheckModeBuiltin(listener: () => Promise<void>): () => void {
    this.#checkListeners.add(listener);
    return () => this.#checkListeners.delete(listener);
  }

  start(): Promise<void> {
    if (this.#disposed) throw new Error("NodeProviderConfigRuntime 已 dispose");
    if (this.#startPromise) return this.#startPromise;
    const startPromise = this.configService
      .read()
      .then(() => this.#retireZhipuProviderResidue())
      .then(() => {
        if (this.#disposed) return;
        void this.#checkBackground();
        // Managed Worker 无下载配置也无恢复 owner，不建立周期任务。
        if (
          this.#remoteSynchronizer ||
          this.#modeBuiltinSource instanceof EndpointScopedModeBuiltinSource ||
          this.#checkListeners.size > 0
        ) {
          this.#checkTimer = setInterval(() => {
            void this.#checkBackground();
          }, 60_000);
          this.#checkTimer.unref?.();
        }
      });
    this.#startPromise = startPromise;
    void startPromise.catch(() => {
      if (this.#startPromise === startPromise) this.#startPromise = null;
    });
    return startPromise;
  }

  refreshModeBuiltin(options?: { readonly force?: boolean }): Promise<ModeBuiltinRefreshResult> {
    if (this.#disposed) return Promise.resolve("disposed");
    if (this.#modeBuiltinSource instanceof EndpointScopedModeBuiltinSource) {
      return this.#modeBuiltinSource.refresh(options);
    }
    return this.#remoteSynchronizer?.refresh(options) ?? Promise.resolve("skipped");
  }

  /**
   * 去智谱化的一次性对账：把 Personal 层里指向已下线 account:* Provider 的残留条目清掉，
   * 并把 defaultModelSelection 迁到同族保留的 api-key 标准预设。
   *
   * 挂在 Config Runtime 启动边界而不是 UI：desktop / CLI / server 共用这一个 Personal
   * Repository 所有者，放 UI 会留下「只有渲染进程才迁移」的缺口。写入必须走
   * `personalRepository.update()`，它才有文件锁与失效通知。
   */
  async #retireZhipuProviderResidue(): Promise<void> {
    if (this.#disposed) return;
    try {
      const modeBuiltin = await this.#modeBuiltinSource.read();
      // 先按当前快照空跑一次：绝大多数启动在这里就返回，不产生文件 IO 与失效通知。
      if (
        !createRetiredZhipuProviderMigrationUpdate(
          await this.#personalRepository.read(),
          modeBuiltin,
        )
      )
        return;
      // 真正写入时从锁内快照重新判定，避免用加锁前的旧判定覆盖其他 writer 的写入。
      await this.#personalRepository.update(
        (current) => createRetiredZhipuProviderMigrationUpdate(current, modeBuiltin) ?? current,
      );
    } catch (error) {
      // 迁移失败不能阻断启动：数据本身仍可加载，悬空条目只是不可用而非非法。
      // 迁移幂等，下次启动会重试。
      this.#onRetiredProviderMigrationError?.(error);
    }
  }

  #checkBackground(): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    if (this.#checkInFlight) return this.#checkInFlight;
    const check = Promise.allSettled([
      this.refreshModeBuiltin(),
      ...[...this.#checkListeners].map((listener) => Promise.resolve().then(listener)),
    ])
      .then((results) => {
        if (this.#disposed) return;
        for (const result of results)
          if (result.status === "rejected") this.#onRemoteRefreshError?.(result.reason);
      })
      .finally(() => {
        if (this.#checkInFlight === check) this.#checkInFlight = null;
      });
    this.#checkInFlight = check;
    return check;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#checkTimer) clearInterval(this.#checkTimer);
    this.#checkTimer = null;
    this.#checkListeners.clear();
    this.#remoteSynchronizer?.dispose();
    this.configService.dispose();
    this.#personalRepository.dispose();
    this.#modeBuiltinSource.dispose();
  }
}

export function createNodeProviderConfigRuntime(
  options: NodeProviderConfigRuntimeOptions,
): NodeProviderConfigRuntime {
  return new NodeProviderConfigRuntime(options);
}
