// 插件宿主 sidecar 骨架（M3 生态域 1 · Task 2）。
//
// 运行（开发形态）：
//   pnpm exec tsx tools/plugin-host/src/main.ts
//
// 协议：一行一帧 NDJSON，方法集合与 `crates/plugin-host/src/protocol.rs` 一一对应。
// 当前是骨架：`plugin/listTools` 返回空列表，`plugin/callTool` 只支持 `echo`。
// 插件/MCP/Skills 的真实发现与执行在 Task 3–5 落地。
import { createInterface } from "node:readline";

const PROTOCOL_VERSION = 1;
const PROTOCOL_ERROR_ID = 0;

type ToolDeclaration = {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
};

const tools: ToolDeclaration[] = [
  {
    name: "echo",
    description: "Echo the given text back (fixture tool for the sidecar skeleton).",
    inputSchema: {
      type: "object",
      properties: { text: { type: "string" } },
      required: ["text"],
      additionalProperties: false,
    },
  },
];

function write(frame: unknown): void {
  process.stdout.write(`${JSON.stringify(frame)}\n`);
}

function ok(id: number, result: unknown): void {
  write({ id, result });
}

function fail(id: number, code: number, message: string): void {
  write({ id, error: { code, message } });
}

const input = createInterface({ input: process.stdin });

input.on("line", (line) => {
  if (line.trim() === "") return;
  let frame: Record<string, unknown>;
  try {
    frame = JSON.parse(line) as Record<string, unknown>;
  } catch {
    fail(PROTOCOL_ERROR_ID, -32700, "parse error");
    return;
  }
  const id = typeof frame.id === "number" ? frame.id : undefined;
  const method = typeof frame.method === "string" ? frame.method : undefined;
  if (method === undefined) {
    fail(id ?? PROTOCOL_ERROR_ID, -32600, "invalid request");
    return;
  }
  if (id === undefined) {
    fail(PROTOCOL_ERROR_ID, -32600, "notification is not supported for this method");
    return;
  }
  const params = (frame.params ?? {}) as Record<string, unknown>;

  switch (method) {
    case "host/hello":
      ok(id, { protocolVersion: PROTOCOL_VERSION, tools: tools.length });
      return;
    case "plugin/listTools":
      ok(id, { tools });
      return;
    case "plugin/health":
      ok(id, { ok: true, tools: tools.length });
      return;
    case "plugin/callTool": {
      const toolName = String(params.toolName ?? "");
      if (toolName !== "echo") {
        fail(id, -32601, `Tool not found: ${toolName}`);
        return;
      }
      const toolInput = (params.input ?? {}) as Record<string, unknown>;
      ok(id, { content: String(toolInput.text ?? "") });
      return;
    }
    case "host/shutdown":
      ok(id, {});
      setImmediate(() => process.exit(0));
      return;
    default:
      fail(id, -32601, `method not found: ${method}`);
  }
});
