// M2 Task 0 Step 4b：对拍用的本地 mock provider。
//
// 为什么不去改 provider 层：`packages/provider-node/src/**` 在上游基线里存在（17 个文件），
// 属上游衍生，clean-room 下只能读接口面（见 docs/specs/clean-room-policy.md）。
// 改成「本地 HTTP 服务扮演 provider 端点 + 配置把运行时指过去」是纯配置 + 黑盒，
// 且默认不可达：`oracle-mock` 只存在于本模块写出的临时配置文件里。
//
// 确定性口径（spec §4.3）：应答是**请求内容的纯函数**——
// index = 请求 messages 里 role==="assistant" 的条数；剧本越界则回最后一步。
// 不用计数器、不用随机数，重试同输入得同输出。
//
// 字段形状依据（接口面）：
//   packages/provider/dist/config/rule-data-schema.d.ts（personal provider/model rules）
//   packages/provider-node/dist/provider-config-file-codec.d.ts（配置文件的 encoding 形状）
import { createServer, type Server } from "node:http";
import { writeFile } from "node:fs/promises";

/** providerId 只存在于 driver 写出的配置文件里，不进入任何生产默认路径。 */
export const ORACLE_MOCK_PROVIDER_ID = "oracle-mock";
export const ORACLE_MOCK_MODEL_ID = "mock-1";

export type MockToolCall = { name: string; input: Record<string, unknown> };
/** 真正写进 SSE 的一步：要么出工具调用，要么出终结文本。 */
export type MockStepBody = { toolCalls: MockToolCall[] } | { text: string };
/**
 * 剧本一步：静态正文，或由**请求内容**决定正文的纯函数。
 *
 * 后者的存在理由：有些工具的参数只有在运行时才产生（例如后台任务的 `task_id`），
 * 静态剧本写不出来。函数仍满足「同输入得同输出」的确定性口径（spec §4.3）。
 */
export type MockStep = MockStepBody | ((body: AnthropicBody) => MockStepBody);

export type MockProvider = {
  /** 形如 `http://127.0.0.1:<port>`，运行时据此拼 `/v1/messages`。 */
  readonly baseUrl: string;
  /** 收到的请求路径，供用例断言「运行时确实打到 mock 而不是真实端点」。 */
  readonly requestPaths: string[];
  close(): Promise<void>;
};

type AnthropicBody = {
  stream?: boolean;
  tools?: unknown[];
  messages?: { role?: string; content?: unknown }[];
};

/** 剧本下标：请求里已完成的 assistant 轮数。纯函数，重试幂等。 */
export function scriptIndexForRequest(body: AnthropicBody): number {
  const messages = Array.isArray(body.messages) ? body.messages : [];
  return messages.filter((message) => message.role === "assistant").length;
}

/** 请求 t 是否消费剧本：带 tools 的才是会话主请求；标题生成之类的请求不消费。 */
export function usesScript(body: AnthropicBody): boolean {
  return Array.isArray(body.tools) && body.tools.length > 0;
}

function sse(res: import("node:http").ServerResponse, event: string, data: unknown): void {
  res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
}

function writeSseResponse(
  res: import("node:http").ServerResponse,
  step: MockStepBody,
  index: number,
  onContentBlockDeltaWrite?: (at: number) => void,
): void {
  res.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
  });
  sse(res, "message_start", {
    type: "message_start",
    message: {
      id: `msg_mock_${index}`,
      type: "message",
      role: "assistant",
      model: ORACLE_MOCK_MODEL_ID,
      content: [],
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 1 },
    },
  });
  if ("toolCalls" in step) {
    step.toolCalls.forEach((call, position) => {
      sse(res, "content_block_start", {
        type: "content_block_start",
        index: position,
        content_block: {
          type: "tool_use",
          id: `toolu_mock_${index}_${position}`,
          name: call.name,
          input: {},
        },
      });
      sse(res, "content_block_delta", {
        type: "content_block_delta",
        index: position,
        delta: { type: "input_json_delta", partial_json: JSON.stringify(call.input) },
      });
      onContentBlockDeltaWrite?.(Date.now());
      sse(res, "content_block_stop", { type: "content_block_stop", index: position });
    });
    sse(res, "message_delta", {
      type: "message_delta",
      delta: { stop_reason: "tool_use", stop_sequence: null },
      usage: { output_tokens: 20 },
    });
  } else {
    sse(res, "content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" },
    });
    sse(res, "content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text: step.text },
    });
    onContentBlockDeltaWrite?.(Date.now());
    sse(res, "content_block_stop", { type: "content_block_stop", index: 0 });
    sse(res, "message_delta", {
      type: "message_delta",
      delta: { stop_reason: "end_turn", stop_sequence: null },
      usage: { output_tokens: 12 },
    });
  }
  sse(res, "message_stop", { type: "message_stop" });
  res.end();
}

/** 非流式（标题生成等）回一个固定文本的普通报文，不消费剧本。 */
function writeJsonResponse(res: import("node:http").ServerResponse): void {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(
    JSON.stringify({
      id: "msg_mock_ancillary",
      type: "message",
      role: "assistant",
      model: ORACLE_MOCK_MODEL_ID,
      content: [{ type: "text", text: "oracle-mock" }],
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 1, output_tokens: 1 },
    }),
  );
}

export type MockProviderOptions = {
  /**
   * 纯观测钩子：每个请求体解析后回调一次。
   *
   * 存在的理由（Task 2）：模型侧的**工具清单**（`tools[].name/description/input_schema`）
   * 只存在于这个请求里——它是运行时真正对外声明的工具接口面，黑盒观察它比重述一遍强。
   * 钩子是只读观察，抛异常会原样冒到 HTTP 处理路径，不吞不掩。
   */
  onRequestBody?: (path: string, body: AnthropicBody) => void;
  /**
   * 纯观测钩子：每写出一个 `content_block_delta`（SSE）前回调一次，参数是写出时刻。
   *
   * 存在的理由（M4a stream-latency bench）：测「模型端点字节 → agent 事件出口」的
   * 转发延迟需要一个**发送侧时间戳**；与 `onRequestBody` 同模式——只读观察，
   * 抛异常原样冒出。时刻由调用方传入的时钟决定（bench 用 `performance.now`）。
   */
  onContentBlockDeltaWrite?: (at: number) => void;
};

export async function startMockProvider(
  script: MockStep[],
  options: MockProviderOptions = {},
): Promise<MockProvider> {
  const requestPaths: string[] = [];
  const server: Server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => {
      body += String(chunk);
    });
    request.on("end", () => {
      requestPaths.push(request.url ?? "");
      let parsed: AnthropicBody = {};
      try {
        parsed = JSON.parse(body) as AnthropicBody;
      } catch {
        parsed = {};
      }
      options.onRequestBody?.(request.url ?? "", parsed);
      if (parsed.stream !== true) {
        writeJsonResponse(response);
        return;
      }
      if (!usesScript(parsed)) {
        writeJsonResponse(response);
        return;
      }
      const index = scriptIndexForRequest(parsed);
      const entry = script[Math.min(index, script.length - 1)];
      const step = typeof entry === "function" ? entry(parsed) : entry;
      if (!step) {
        // 剧本为空是调用方的错（夹具必须给剧本），显式失败而不是回一个空事件流。
        response.writeHead(500, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: "oracle mock provider: empty script" }));
        return;
      }
      writeSseResponse(response, step, index, options.onContentBlockDeltaWrite);
    });
  });

  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("oracle mock provider: 未能取得监听端口");
  }
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    requestPaths,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}

/**
 * 写出 driver 自己的 personal provider config，并把运行时指向它。
 * 绝不继承调用方的 `MODE_PERSONAL_PROVIDER_CONFIG_FILE`——开发机上它指向真实凭据文件。
 */
export async function writeOracleProviderConfig(filePath: string, baseUrl: string): Promise<void> {
  const config = {
    schemaVersion: 1,
    config: {
      providerConfigRules: {
        providerRules: [
          {
            providerId: ORACLE_MOCK_PROVIDER_ID,
            providerName: "Oracle Mock",
            enabled: true,
            config: {
              group: "standard-personal",
              visibility: "visible",
              access: { type: "api-key", apiKey: "oracle-mock-key" },
              personalModelIds: [ORACLE_MOCK_MODEL_ID],
              api: { type: "anthropic-messages", baseUrl },
            },
          },
        ],
      },
      modelConfigRules: {
        providerModelRules: [
          {
            providerId: ORACLE_MOCK_PROVIDER_ID,
            modelId: ORACLE_MOCK_MODEL_ID,
            config: {
              properties: {
                contextWindow: 200000,
                supportsToolCall: true,
                supportsJsonSchemaOutput: false,
                supportsNativeWebSearch: false,
                supportsMidConversationSystem: true,
                inputFormat: { supportsText: true, supportsImage: false },
                outputFormat: { supportsText: true },
              },
              optionSpecs: {
                reasoningLevel: { values: ["disabled", "enabled"] },
                maxOutputTokens: { max: 8192 },
              },
            },
          },
        ],
        manualProviderModelRules: [],
      },
      providerOrder: [ORACLE_MOCK_PROVIDER_ID],
    },
  };
  await writeFile(filePath, `${JSON.stringify(config, null, 2)}\n`, "utf8");
}

/** 把剧本里的 `${cwd}` 占位符换成实际工作目录（夹具因此可跨机器回放）。 */
export function substituteCwd(script: MockStep[], cwd: string): MockStep[] {
  const walk = (value: unknown): unknown => {
    if (typeof value === "string") {
      return value.replaceAll("${cwd}", cwd);
    }
    if (Array.isArray(value)) {
      return value.map(walk);
    }
    if (value !== null && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([key, inner]) => [key, walk(inner)]),
      );
    }
    return value;
  };
  return walk(script) as MockStep[];
}
