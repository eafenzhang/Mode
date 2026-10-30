/* oxlint-disable eslint(max-lines) -- 对拍数据集中声明合法/恢复/回落样例，拆分会降低可核对性。 */
/**
 * 生成 `provider-config-source` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS，全部通过真实实现执行）：
 * - `packages/provider-node/src/personal-provider-config-repository.ts`
 * - `packages/provider-node/src/mode-builtin-provider-config-source.ts`
 * - `packages/provider/src/config-service.ts`
 *
 * ```bash
 * pnpm provider-config-source:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p provider-config-source"
 * ```
 *
 * fixture 里所有绝对路径都以 `<ROOT>` 占位；Builtin revision 内的 sourceKey 以
 * `<SOURCE_KEY>` 占位。Rust 侧在自建临时目录上重建同样的相对布局，计算自己的
 * sourceKey 后替换占位再逐字节比较。
 */
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { ProviderConfigService } from "../packages/provider/src/config-service.js";
import { NodeModeBuiltinProviderConfigSource } from "../packages/provider-node/src/mode-builtin-provider-config-source.js";
import { NodePersonalProviderConfigRepository } from "../packages/provider-node/src/personal-provider-config-repository.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-config-source",
  "fixtures",
  "provider-config-source.json",
);
const ROOT_PLACEHOLDER = "<ROOT>";
const SOURCE_KEY_PLACEHOLDER = "<SOURCE_KEY>";

interface FileRecord {
  path: string;
  content: string;
}

interface PersonalCase {
  name: string;
  filePath: string;
  files: FileRecord[];
  expectedJson: string;
}

interface BuiltinCase {
  name: string;
  bundled: string;
  active: string | null;
  files: FileRecord[];
  expectedTemplate: string | null;
  expectError: boolean;
}

interface SnapshotCase {
  name: string;
  bundled: string;
  active: string | null;
  personalPath: string;
  files: FileRecord[];
  expectedTemplate: string;
}

interface ResolveCase {
  name: string;
  cwd: string;
  input: string;
  expectedResolved: string;
  expectedSourceKey: string;
}

const workRoot = mkdtempSync(path.join(os.tmpdir(), "provider-config-source-fixtures-"));
const personalCases: PersonalCase[] = [];
const builtinCases: BuiltinCase[] = [];
const snapshotCases: SnapshotCase[] = [];

function sha256(text: string): string {
  return createHash("sha256").update(text).digest("hex");
}

function writeFiles(dir: string, files: FileRecord[]): void {
  for (const file of files) {
    const target = path.join(dir, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content, "utf8");
  }
}

function caseDir(name: string): string {
  const dir = path.join(workRoot, name.replace(/[^a-zA-Z0-9_-]+/g, "_"));
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  return dir;
}

// ---------------------------------------------------------------------------
// 输入构造
// ---------------------------------------------------------------------------

const CURRENT_MANUAL_CONFIG = {
  enabled: true,
  properties: {
    contextWindow: 64_000,
    supportsJsonSchemaOutput: true,
    supportsNativeWebSearch: false,
    supportsMidConversationSystem: true,
    inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: true },
  },
  optionSpecs: {
    reasoningLevel: { values: ["low", "high"], map: '{"effort": reasoningLevel}' },
    maxOutputTokens: { max: 8_192 },
  },
};

const LEGACY_COMPLETE_MANUAL_CONFIG = {
  enabled: true,
  properties: {
    requiresMfjsToolSchema: false,
    contextWindow: 128_000,
    inputFormat: {
      supportsText: true,
      supportsImage: true,
      supportsVideo: false,
      supportsAudio: false,
      supportsPdf: true,
    },
    outputFormat: { supportsText: true },
    supportsToolCall: true,
    supportsJsonSchemaOutput: true,
    supportsNativeWebSearch: false,
    supportsMidConversationSystem: true,
  },
  optionSpecs: {
    reasoningLevel: { values: ["low", "high"], map: '{"effort": reasoningLevel}' },
    maxOutputTokens: { max: 4_096, map: '{"tokens": maxOutputTokens}' },
  },
};

const LEGACY_EDITABLE_MANUAL_CONFIG = {
  enabled: true,
  properties: {
    requiresMfjsToolSchema: false,
    contextWindow: 96_000,
    supportsJsonSchemaOutput: true,
    supportsNativeWebSearch: false,
    supportsMidConversationSystem: true,
    inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: true },
  },
  optionSpecs: {
    reasoningLevel: { values: ["low", "medium", "high"], map: '{"effort": reasoningLevel}' },
    maxOutputTokens: { max: 2_048 },
  },
};

const PERSONAL_PROVIDER_RULES = {
  providerRules: [
    {
      providerId: "z-personal",
      providerName: "Z Personal",
      enabled: true,
      config: {
        group: "standard-personal",
        api: { type: "openai-responses", baseUrl: "https://z.example.com" },
        access: { type: "api-key", apiKey: "sk-z" },
        personalModelIds: ["m-z"],
        modelOrder: ["m-z"],
        visibility: "visible",
      },
    },
    { providerId: "a-personal", config: { group: "standard-personal" } },
  ],
};

const PERSONAL_MODEL_RULES = {
  providerModelRules: [
    {
      providerId: "z-personal",
      modelId: "m-z",
      config: { enabled: true, properties: { contextWindow: 200_000 } },
    },
  ],
  manualProviderModelRules: [
    { providerId: "a-personal", modelId: "m-manual", config: CURRENT_MANUAL_CONFIG },
  ],
};

function personalDocument(overrides: Record<string, unknown> = {}): unknown {
  return {
    schemaVersion: 1,
    config: {
      providerOrder: ["z-personal", "a-personal"],
      providerConfigRules: PERSONAL_PROVIDER_RULES,
      modelConfigRules: PERSONAL_MODEL_RULES,
      ...overrides,
    },
  };
}

function builtinRelease(revision: number, extraModelRules: unknown = {}): unknown {
  return {
    schemaVersion: 1,
    revision,
    config: {
      providerConfigRules: {
        templateRules: [
          {
            templateId: "tpl-1",
            templateNameMap: { "en-US": "Template 1", "zh-CN": "模板 1" },
            config: {
              logo: { type: "builtin", key: "k" },
              api: { type: "openai-responses", baseUrl: "https://api.example.com" },
              builtinModelIds: ["m1"],
            },
          },
        ],
        providerRules: [
          {
            providerId: "builtin:p1",
            providerName: "P1",
            enabled: true,
            config: {
              group: "zai-family",
              api: { type: "openai-responses", baseUrl: "https://api.example.com" },
              builtinModelIds: ["m1"],
              visibility: "visible",
            },
          },
          { providerId: "builtin:p2", config: { group: "bigmodel-family" } },
        ],
      },
      modelConfigRules: {
        modelRules: [{ modelMatch: "gpt-.*", config: { enabled: true } }],
        modelApiRules: [],
        providerSiteRules: [],
        templateModelRules: [],
        builtinProviderModelRules: [
          {
            providerId: "builtin:p1",
            modelId: "m1",
            config: { enabled: true, properties: { contextWindow: 128_000 } },
          },
        ],
        ...(extraModelRules as Record<string, unknown>),
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Personal
// ---------------------------------------------------------------------------

async function runPersonal(name: string, file: FileRecord | null): Promise<void> {
  const dir = caseDir(`personal-${name}`);
  if (file) writeFiles(dir, [file]);
  const filePath = path.join(dir, "provider_config.json");
  const repository = new NodePersonalProviderConfigRepository({
    filePath,
    pollingIntervalMs: false,
  });
  try {
    const snapshot = await repository.read();
    personalCases.push({
      name,
      filePath: "provider_config.json",
      files: file ? [{ path: "provider_config.json", content: file.content }] : [],
      expectedJson: JSON.stringify(snapshot),
    });
  } finally {
    repository.dispose();
  }
}

await runPersonal("valid-full", {
  path: "provider_config.json",
  content: JSON.stringify(personalDocument({ defaultModelSelection: {
    providerId: "z-personal",
    modelId: "m-z",
    options: { reasoningLevel: "high" },
  } })),
});
await runPersonal("missing", null);
await runPersonal("corrupt-json", {
  path: "provider_config.json",
  content: "{not json",
});
await runPersonal("schema-too-high", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 2,
    config: {
      providerConfigRules: { providerRules: [] },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
    },
  }),
});
await runPersonal("schema-too-low", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 0,
    config: {
      providerConfigRules: { providerRules: [] },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
    },
  }),
});
await runPersonal("schema-missing", {
  path: "provider_config.json",
  content: JSON.stringify({
    config: {
      providerConfigRules: { providerRules: [] },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
    },
  }),
});
await runPersonal("legacy-complete-manual", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerConfigRules: PERSONAL_PROVIDER_RULES,
      modelConfigRules: {
        providerModelRules: PERSONAL_MODEL_RULES.providerModelRules,
        manualProviderModelRules: [
          { providerId: "a-personal", modelId: "m-manual", config: LEGACY_COMPLETE_MANUAL_CONFIG },
        ],
      },
    },
  }),
});
await runPersonal("legacy-editable-manual", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerConfigRules: PERSONAL_PROVIDER_RULES,
      modelConfigRules: {
        providerModelRules: PERSONAL_MODEL_RULES.providerModelRules,
        manualProviderModelRules: [
          { providerId: "a-personal", modelId: "m-manual", config: LEGACY_EDITABLE_MANUAL_CONFIG },
        ],
      },
    },
  }),
});
await runPersonal("order-and-selection", {
  path: "provider_config.json",
  content: JSON.stringify(personalDocument({
    providerOrder: ["a-personal", "z-personal"],
    defaultModelSelection: { providerId: "a-personal", modelId: "m-manual" },
  })),
});
await runPersonal("selection-trimmed", {
  path: "provider_config.json",
  content: JSON.stringify(personalDocument({
    defaultModelSelection: {
      providerId: " z-personal ",
      modelId: " m-z ",
      options: { reasoningLevel: " high " },
    },
  })),
});
await runPersonal("selection-empty-options", {
  path: "provider_config.json",
  content: JSON.stringify(personalDocument({
    defaultModelSelection: { providerId: "z-personal", modelId: "m-z", options: {} },
  })),
});
await runPersonal("non-canonical-key-order", {
  path: "provider_config.json",
  // 键顺序与规范编码不同，读取应返回规范快照。
  content: `{
  "config": {
    "modelConfigRules": ${JSON.stringify(PERSONAL_MODEL_RULES)},
    "providerConfigRules": ${JSON.stringify(PERSONAL_PROVIDER_RULES)},
    "providerOrder": ["z-personal", "a-personal"]
  },
  "schemaVersion": 1
}`,
});
await runPersonal("invalid-provider-rule", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerConfigRules: {
        providerRules: [{ providerId: "p", config: { group: "zai-family" } }],
      },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
    },
  }),
});
await runPersonal("null-provider-order", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerOrder: null,
      providerConfigRules: { providerRules: [] },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
    },
  }),
});
await runPersonal("unknown-config-key", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerConfigRules: { providerRules: [] },
      modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
      surprise: true,
    },
  }),
});
await runPersonal("invalid-manual-preserved", {
  path: "provider_config.json",
  content: JSON.stringify({
    schemaVersion: 1,
    config: {
      providerConfigRules: { providerRules: [] },
      modelConfigRules: {
        providerModelRules: [],
        manualProviderModelRules: [{ providerId: "p", modelId: "m", config: {} }],
      },
    },
  }),
});

// ---------------------------------------------------------------------------
// Builtin
// ---------------------------------------------------------------------------

async function runBuiltin(
  name: string,
  options: {
    files: FileRecord[];
    bundledLiteral: string;
    activeLiteral: string | null;
    expectError?: boolean;
  },
): Promise<void> {
  const dir = caseDir(`builtin-${name}`);
  writeFiles(dir, options.files);
  const substitute = (value: string) => value.split(ROOT_PLACEHOLDER).join(dir);
  const bundled = substitute(options.bundledLiteral);
  const active = options.activeLiteral === null ? undefined : substitute(options.activeLiteral);
  const source = new NodeModeBuiltinProviderConfigSource({
    bundledFilePath: bundled,
    activeFilePath: active,
    watch: false,
  });
  let expectedTemplate: string | null = null;
  let expectError = options.expectError ?? false;
  try {
    const snapshot = await source.read();
    // 与 source 构造器一致：trim 后为空则回落到 bundled 的 trim 结果。
    const activePath = active?.trim() || bundled.trim();
    const sourceKey = sha256(path.resolve(activePath));
    expectedTemplate = JSON.stringify(snapshot).split(sourceKey).join(SOURCE_KEY_PLACEHOLDER);
  } catch {
    expectError = true;
  } finally {
    source.dispose();
  }
  builtinCases.push({
    name,
    bundled: options.bundledLiteral,
    active: options.activeLiteral,
    files: options.files,
    expectedTemplate,
    expectError,
  });
}

const bundledFile = (revision: number, extra?: unknown): FileRecord => ({
  path: "bundled.json",
  content: JSON.stringify(builtinRelease(revision, extra)),
});

await runBuiltin("bundled-valid-active-missing", {
  files: [bundledFile(5)],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/missing/active.json`,
});
await runBuiltin("active-valid-newer", {
  files: [
    bundledFile(5),
    { path: "active/active.json", content: JSON.stringify(builtinRelease(7)) },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
});
await runBuiltin("active-valid-older", {
  files: [
    bundledFile(9),
    { path: "active/active.json", content: JSON.stringify(builtinRelease(3)) },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
});
await runBuiltin("active-corrupt", {
  files: [
    bundledFile(5),
    { path: "active/active.json", content: "{broken" },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
});
await runBuiltin("bundled-corrupt-active-valid", {
  files: [
    { path: "bundled.json", content: "{broken" },
    { path: "active/active.json", content: JSON.stringify(builtinRelease(4)) },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
});
await runBuiltin("both-invalid", {
  files: [
    { path: "bundled.json", content: "{}" },
    { path: "active/active.json", content: "{}" },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
  expectError: true,
});
await runBuiltin("same-revision-conflict", {
  files: [
    bundledFile(5),
    {
      path: "active/active.json",
      content: JSON.stringify(
        builtinRelease(5, {
          // 同 revision 不同内容：Active 多一条 provider-site 规则。
          providerSiteRules: [
            {
              modelMatch: "claude-.*",
              baseUrlMatch: "https://api\\\\.anthropic\\\\.com",
              config: { enabled: true },
            },
          ],
        }),
      ),
    },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
});
await runBuiltin("no-active-option", {
  files: [bundledFile(6)],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: null,
});
await runBuiltin("empty-active", {
  files: [bundledFile(6)],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: "   ",
});
await runBuiltin("padded-paths", {
  files: [
    bundledFile(5),
    { path: "active/active.json", content: JSON.stringify(builtinRelease(8)) },
  ],
  bundledLiteral: `  ${ROOT_PLACEHOLDER}/bundled.json  `,
  activeLiteral: `  ${ROOT_PLACEHOLDER}/active/active.json  `,
});
await runBuiltin("active-path-normalized", {
  files: [
    bundledFile(5),
    { path: "sub/active.json", content: JSON.stringify(builtinRelease(8)) },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/sub/../sub/./active.json`,
});
await runBuiltin("retired-zapi", {
  files: [
    {
      path: "bundled.json",
      content: JSON.stringify({
        schemaVersion: 1,
        revision: 1,
        config: {
          providerConfigRules: {
            templateRules: [],
            providerRules: [{ providerId: "builtin:zapi", config: { group: "zai-family" } }],
          },
          modelConfigRules: {
            modelRules: [],
            modelApiRules: [],
            providerSiteRules: [],
            templateModelRules: [],
            builtinProviderModelRules: [],
          },
        },
      }),
    },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: null,
  expectError: true,
});

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

async function runSnapshot(
  name: string,
  options: {
    files: FileRecord[];
    bundledLiteral: string;
    activeLiteral: string | null;
    personalPath: string;
  },
): Promise<void> {
  const dir = caseDir(`snapshot-${name}`);
  writeFiles(dir, options.files);
  const substitute = (value: string) => value.split(ROOT_PLACEHOLDER).join(dir);
  const bundled = substitute(options.bundledLiteral);
  const active = options.activeLiteral === null ? undefined : substitute(options.activeLiteral);
  const personalPath = path.join(dir, options.personalPath);
  const source = new NodeModeBuiltinProviderConfigSource({
    bundledFilePath: bundled,
    activeFilePath: active,
    watch: false,
  });
  const repository = new NodePersonalProviderConfigRepository({
    filePath: personalPath,
    pollingIntervalMs: false,
  });
  const service = new ProviderConfigService({
    modeBuiltinSource: source,
    personalRepository: repository,
  });
  try {
    const snapshot = await service.read();
    // 与 source 构造器一致：trim 后为空则回落到 bundled 的 trim 结果。
    const activePath = active?.trim() || bundled.trim();
    const sourceKey = sha256(path.resolve(activePath));
    const expectedTemplate = JSON.stringify(snapshot)
      .split(sourceKey)
      .join(SOURCE_KEY_PLACEHOLDER);
    snapshotCases.push({
      name,
      bundled: options.bundledLiteral,
      active: options.activeLiteral,
      personalPath: options.personalPath,
      files: options.files,
      expectedTemplate,
    });
  } finally {
    service.dispose();
  }
}

await runSnapshot("valid-both", {
  files: [
    bundledFile(5),
    {
      path: "personal/provider_config.json",
      content: JSON.stringify(personalDocument({
        defaultModelSelection: { providerId: "z-personal", modelId: "m-z" },
      })),
    },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
  personalPath: "personal/provider_config.json",
});
await runSnapshot("personal-missing", {
  files: [bundledFile(5)],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: null,
  personalPath: "personal/provider_config.json",
});
await runSnapshot("personal-corrupt", {
  files: [
    bundledFile(5),
    { path: "personal/provider_config.json", content: "{broken" },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: null,
  personalPath: "personal/provider_config.json",
});
await runSnapshot("active-newer", {
  files: [
    bundledFile(5),
    { path: "active/active.json", content: JSON.stringify(builtinRelease(8)) },
    {
      path: "personal/provider_config.json",
      content: JSON.stringify(personalDocument()),
    },
  ],
  bundledLiteral: `${ROOT_PLACEHOLDER}/bundled.json`,
  activeLiteral: `${ROOT_PLACEHOLDER}/active/active.json`,
  personalPath: "personal/provider_config.json",
});

// ---------------------------------------------------------------------------
// Resolve / sourceKey
// ---------------------------------------------------------------------------

const resolveInputs: Array<{ name: string; cwd: string; input: string }> = [
  {
    name: "absolute-forward",
    cwd: "C:/work/base",
    input: "C:/work/base/provider/active.json",
  },
  {
    name: "dot-segment",
    cwd: "C:/work/base",
    input: "C:\\work\\base\\provider\\.\\active.json",
  },
  {
    name: "parent-segment",
    cwd: "C:/work/base",
    input: "C:/work/base/provider/sub/../active.json",
  },
  {
    name: "lowercase-drive",
    cwd: "C:/work/base",
    input: "c:/work/base/active.json",
  },
  {
    name: "duplicate-separators",
    cwd: "C:/work/base",
    input: "C:/work//base///active.json",
  },
  {
    name: "trailing-separator",
    cwd: "C:/work/base",
    input: "C:/work/base/provider/",
  },
  {
    name: "relative",
    cwd: "C:/work/base",
    input: "provider/active.json",
  },
  {
    name: "relative-parent",
    cwd: "C:/work/base/sub",
    input: "../active.json",
  },
  {
    name: "parent-above-root",
    cwd: "C:/work/base",
    input: "C:/../x.json",
  },
];

const resolveCases: ResolveCase[] = resolveInputs.map((entry) => {
  const expectedResolved = path.resolve(entry.cwd, entry.input);
  return {
    ...entry,
    expectedResolved,
    expectedSourceKey: sha256(expectedResolved),
  };
});

// ---------------------------------------------------------------------------
// 输出
// ---------------------------------------------------------------------------

const fixture = {
  version: 1,
  generatedBy: "scripts/generate-provider-config-source-fixtures.ts",
  platform: process.platform,
  rootPlaceholder: ROOT_PLACEHOLDER,
  sourceKeyPlaceholder: SOURCE_KEY_PLACEHOLDER,
  resolveCases,
  personal: personalCases,
  builtin: builtinCases,
  snapshot: snapshotCases,
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");

if (existsSync(workRoot)) rmSync(workRoot, { recursive: true, force: true });
console.log(
  `wrote ${personalCases.length} personal, ${builtinCases.length} builtin, ${snapshotCases.length} snapshot, ${resolveCases.length} resolve cases to ${OUTPUT}`,
);
