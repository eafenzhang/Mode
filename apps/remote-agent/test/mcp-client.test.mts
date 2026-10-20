import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";
import test from "node:test";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const binary = process.env.MODE_AGENT_BIN ?? join(repoRoot, "apps/remote-agent/dist/mode-agent-xp-x86.exe");
const skip = !existsSync(binary);

function waitForListening(child: ChildProcessWithoutNullStreams): Promise<string> {
  return new Promise((resolveAddress, reject) => {
    const timer = setTimeout(() => reject(new Error("agent did not report listening address")), 10_000);
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      if (line.startsWith("listening=")) {
        clearTimeout(timer);
        resolveAddress(line.slice("listening=".length));
      }
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => { stderr += chunk.toString(); });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`agent exited before listening: ${code}
${stderr}`));
    });
  });
}

test("Mode MCP client can initialize, list tools, and execute on mode-agent", { skip, timeout: 45_000 }, async (t) => {
  const tempDir = mkdtempSync(join(tmpdir(), "mode-agent-mcp-"));
  const token = "test-token-0123456789";
  const configPath = join(tempDir, "mode-agent.json");
  writeFileSync(
    configPath,
    JSON.stringify(
      {
        bind: "127.0.0.1",
        port: 47821,
        token,
        dataDir: tempDir,
        allowShell: true,
        allowWrite: false,
        allowDelete: false,
        allowDesktopControl: true,
        allowDb: false,
        allowDbWrite: false,
        dsns: {},
      },
      null,
      2,
    ),
  );

  const child = spawn(binary, ["serve", "--config", configPath, "--port", "0"], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let client: Client | undefined;

  t.after(async () => {
    if (client) {
      await client.close().catch(() => undefined);
    }
    if (!child.killed) {
      child.kill();
    }
    await new Promise((resolveExit) => setTimeout(resolveExit, 300));
    rmSync(tempDir, { recursive: true, force: true });
  });

  const address = await waitForListening(child);
  const transport = new StreamableHTTPClientTransport(new URL(`http://${address}/mcp`), {
    requestInit: { headers: { Authorization: `Bearer ${token}` } },
  });
  client = new Client({ name: "mode-agent-test", version: "0.1.0" });
  await client.connect(transport);

  const listed = await client.listTools();
  const names = listed.tools.map((tool) => tool.name);
  for (const expected of ["remote_info", "remote_exec", "remote_fs_list", "remote_screen_capture"]) {
    assert.ok(names.includes(expected), `missing tool ${expected}`);
  }

  const info = await client.callTool({ name: "remote_info", arguments: {} });
  assert.equal(info.isError ?? false, false);
  const infoText = (info.content as Array<{ type: string; text?: string }>)
    .map((item) => item.text ?? "")
    .join("\n");
  assert.match(infoText, /windowsVersion/);

  const exec = await client.callTool({
    name: "remote_exec",
    arguments: { command: "echo mode-agent-mcp-ok", shell: "cmd", timeoutMs: 15_000 },
  });
  assert.equal(exec.isError ?? false, false);
  const execText = (exec.content as Array<{ type: string; text?: string }>)
    .map((item) => item.text ?? "")
    .join("\n");
  assert.match(execText, /mode-agent-mcp-ok/);
  assert.match(execText, /"exitCode":0/);

  const job = await client.callTool({
    name: "remote_job_start",
    arguments: { command: "ping -n 5 127.0.0.1", shell: "cmd" },
  });
  assert.equal(job.isError ?? false, false);
  const jobText = (job.content as Array<{ type: string; text?: string }>)
    .map((item) => item.text ?? "")
    .join("\n");
  const jobId = JSON.parse(jobText).jobId as string;
  assert.match(jobId, /^[0-9a-f]{8,32}$/);
  await new Promise((resolveWait) => setTimeout(resolveWait, 200));
  const status = await client.callTool({ name: "remote_job_status", arguments: { jobId } });
  assert.equal(status.isError ?? false, false);
  const cancel = await client.callTool({ name: "remote_job_cancel", arguments: { jobId } });
  assert.equal(cancel.isError ?? false, false);
  const cancelText = (cancel.content as Array<{ type: string; text?: string }>)
    .map((item) => item.text ?? "")
    .join("\n");
  assert.match(cancelText, /"status":"canceled"/);

  const screen = await client.callTool({
    name: "remote_screen_capture",
    arguments: { maxWidth: 320, maxHeight: 240 },
  });
  assert.equal(screen.isError ?? false, false);
  const image = (screen.content as Array<{ type: string; mimeType?: string; data?: string }>).find(
    (item) => item.type === "image",
  );
  assert.ok(image, "expected an image content block");
  assert.equal(image.mimeType, "image/png");
  assert.ok((image.data ?? "").length > 100);
});
