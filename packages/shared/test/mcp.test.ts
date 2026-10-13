import { describe, expect, it } from "vitest";
import {
  convertToModeAgentMcpServer,
  isModeCuaMcpCommand,
  isModeCuaMcpPackageArg,
} from "../src/mcp.js";

describe("mode-cua MCP matcher (single source of truth for services + CLI bootstrap)", () => {
  it("matches the mode-cua command by leaf name", () => {
    expect(isModeCuaMcpCommand("mode-cua")).toBe(true);
    expect(isModeCuaMcpCommand("/opt/homebrew/bin/mode-cua")).toBe(true);
    expect(isModeCuaMcpCommand("uvx")).toBe(false);
    expect(isModeCuaMcpCommand("mode-cua-server")).toBe(false);
  });

  it("matches uvx/pip package spec forms", () => {
    for (const arg of ["mode-cua", "mode-cua[macos]", "mode-cua@v0.1.3", "mode-cua[macos]@v0.1.3"]) {
      expect(isModeCuaMcpPackageArg(arg)).toBe(true);
    }
  });

  it("matches git and local-path forms (the drift that broke desktop/services parity)", () => {
    expect(isModeCuaMcpPackageArg("mode-cua.git")).toBe(true);
    expect(isModeCuaMcpPackageArg("git+https://github.com/x/mode-cua.git@v0.1.3")).toBe(true);
    expect(isModeCuaMcpPackageArg("/Users/me/src/mode-cua")).toBe(true);
    expect(isModeCuaMcpPackageArg("C:\\src\\mode-cua")).toBe(true);
  });

  it("does not match unrelated args", () => {
    for (const arg of ["--from", "run", "mode-cua-server", "not-mode-cua", "server.js"]) {
      expect(isModeCuaMcpPackageArg(arg)).toBe(false);
    }
  });

  it("recognizes a package spec placed in COMMAND position (command/arg matchers stay consistent)", () => {
    // 之前 command matcher 只认精确叶子 mode-cua，会漏判把包规格直接当 command 的配置 → 漏注入 broker。
    for (const command of ["mode-cua@1.2.3", "mode-cua[macos]", "mode-cua.git", "/opt/tools/mode-cua@1.2.3"]) {
      expect(isModeCuaMcpCommand(command), command).toBe(true);
    }
  });

  it("recognizes pip == pins and the mode_cua underscore distribution name (PyPI treats _ == -)", () => {
    for (const arg of ["mode-cua==1.2.3", "mode_cua", "mode_cua==1.2.3", "mode_cua@v0.1.3"]) {
      expect(isModeCuaMcpPackageArg(arg), arg).toBe(true);
    }
    expect(isModeCuaMcpCommand("mode_cua")).toBe(true);
  });

  it("does not false-positive on sibling packages like mode-cua-proxy", () => {
    for (const value of ["mode-cua-proxy", "mode-cua-proxy@1.0.0", "mode-cua-helper"]) {
      expect(isModeCuaMcpCommand(value), value).toBe(false);
      expect(isModeCuaMcpPackageArg(value), value).toBe(false);
    }
  });

  it("recognizes `python -m mode_cua.server` submodule form (M1 fail-open fix)", () => {
    // `python -m mode_cua.server` → arg 是 `mode_cua.server`；旧 matcher 漏判 → 不注入 broker → fail-open。
    for (const arg of ["mode_cua.server", "mode-cua.server", "mode_cua.main", "mode-cua.__main__"]) {
      expect(isModeCuaMcpPackageArg(arg), arg).toBe(true);
    }
  });

  it("recognizes local paths with a trailing separator (M1 fail-open fix)", () => {
    // `.../mode-cua/` 直接取叶子会得到空串 → 漏判 → fail-open。
    expect(isModeCuaMcpPackageArg("/Users/me/src/mode-cua/")).toBe(true);
    expect(isModeCuaMcpPackageArg("C:\\src\\mode-cua\\")).toBe(true);
    expect(isModeCuaMcpCommand("/opt/homebrew/bin/mode-cua/")).toBe(true);
    // 但带尾斜杠的 sibling 仍不误判。
    expect(isModeCuaMcpPackageArg("/Users/me/src/mode-cua-proxy/")).toBe(false);
  });
});

describe("MCP config helpers", () => {
  it("passes legacy http_headers through to Mode Agent MCP servers", () => {
    expect(
      convertToModeAgentMcpServer("web-search-prime", {
        type: "http",
        url: "https://bigmodel.example.test/mcp",
        timeoutMs: 5000,
        http_headers: {
          Authorization: "Bearer token",
        },
      }),
    ).toEqual({
      name: "web-search-prime",
      type: "http",
      url: "https://bigmodel.example.test/mcp",
      headers: [{ name: "Authorization", value: "Bearer token" }],
      timeoutMs: 5000,
    });
  });

  it("preserves timeoutMs when converting stdio MCP servers for Mode Agent", () => {
    expect(
      convertToModeAgentMcpServer("context7", {
        type: "stdio",
        command: "npx",
        args: ["-y", "@upstash/context7-mcp"],
        timeoutMs: 3,
      }),
    ).toEqual({
      name: "context7",
      command: "npx",
      args: ["-y", "@upstash/context7-mcp"],
      env: [],
      timeoutMs: 3,
    });
  });

  it("preserves timeoutMs when converting sse MCP servers for Mode Agent", () => {
    expect(
      convertToModeAgentMcpServer("stream", {
        type: "sse",
        url: "https://bigmodel.example.test/sse",
        timeoutMs: 8000,
      }),
    ).toEqual({
      name: "stream",
      type: "sse",
      url: "https://bigmodel.example.test/sse",
      headers: [],
      timeoutMs: 8000,
    });
  });

  it("preserves HTTP OAuth client credentials when converting MCP servers for Mode Agent", () => {
    expect(
      convertToModeAgentMcpServer("protected", {
        type: "http",
        url: "https://mcp.example.test/mcp",
        oauth: {
          type: "client_credentials",
          clientId: "mode-client",
          clientSecret: "secret",
          scope: "mcp:tools",
        },
      }),
    ).toMatchObject({
      name: "protected",
      type: "http",
      oauth: {
        type: "client_credentials",
        clientId: "mode-client",
        clientSecret: "secret",
        scope: "mcp:tools",
      },
    });
  });

  it("preserves HTTP OAuth authorization code config when converting MCP servers for Mode Agent", () => {
    expect(
      convertToModeAgentMcpServer("figma", {
        type: "http",
        url: "https://mcp.figma.com/mcp",
        oauth: {
          type: "authorization_code",
          clientName: "Mode",
          redirectPath: "/oauth/callback/mcp/figma",
          scope: "mcp:connect",
        },
      }),
    ).toMatchObject({
      name: "figma",
      type: "http",
      oauth: {
        type: "authorization_code",
        clientName: "Mode",
        redirectPath: "/oauth/callback/mcp/figma",
        scope: "mcp:connect",
      },
    });
  });

  it("does not persist implicit OAuth defaults when converting bare HTTP MCP servers", () => {
    expect(
      convertToModeAgentMcpServer("notion", {
        type: "http",
        url: "https://mcp.notion.com/mcp",
        timeoutMs: 30000,
      }),
    ).toEqual({
      name: "notion",
      type: "http",
      url: "https://mcp.notion.com/mcp",
      headers: [],
      timeoutMs: 30000,
    });
  });

  it("drops invalid timeoutMs when converting MCP servers for Mode Agent", () => {
    expect(
      convertToModeAgentMcpServer("zero", {
        type: "stdio",
        command: "npx",
        timeoutMs: 0,
      }),
    ).not.toHaveProperty("timeoutMs");

    expect(
      convertToModeAgentMcpServer("fractional", {
        type: "http",
        url: "https://bigmodel.example.test/mcp",
        timeoutMs: 1.5,
      }),
    ).not.toHaveProperty("timeoutMs");
  });

  it("keeps headers ahead of legacy http_headers when both are present", () => {
    expect(
      convertToModeAgentMcpServer("web-reader", {
        type: "sse",
        url: "https://bigmodel.example.test/sse",
        headers: {
          Authorization: "Bearer fresh",
        },
        http_headers: {
          Authorization: "Bearer stale",
        },
      }),
    ).toMatchObject({
      headers: [{ name: "Authorization", value: "Bearer fresh" }],
    });
  });
});
