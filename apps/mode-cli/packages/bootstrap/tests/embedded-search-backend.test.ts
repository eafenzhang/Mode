import { describe, expect, it } from "vitest";
import { resolveDefaultEmbeddedSearchBackend } from "../src/app/embedded-search-backend.js";

describe("resolveDefaultEmbeddedSearchBackend", () => {
  it("uses bundled bfs, ugrep, and ripgrep paths by default", () => {
    expect(
      resolveDefaultEmbeddedSearchBackend({
        env: {
          MODE_BFS_BINARY: "/opt/mode/tools/bfs/bfs",
          MODE_RG_BINARY: "/opt/mode/tools/ripgrep/rg",
          MODE_UGREP_BINARY: "/opt/mode/tools/ugrep/ugrep",
        },
      }),
    ).toEqual({
      kind: "native-binaries",
      findCommand: "/opt/mode/tools/bfs/bfs",
      grepCommand: "/opt/mode/tools/ugrep/ugrep",
      rgCommand: "/opt/mode/tools/ripgrep/rg",
    });
  });

  it("falls back to bfs, ugrep, and rg command names when bundled paths are unavailable", () => {
    expect(resolveDefaultEmbeddedSearchBackend({ env: {} })).toEqual({
      kind: "native-binaries",
      findCommand: "bfs",
      grepCommand: "ugrep",
      rgCommand: "rg",
    });
  });

  it("preserves the explicit internal CLI compatibility override", () => {
    expect(
      resolveDefaultEmbeddedSearchBackend({
        env: { MODE_EMBEDDED_SEARCH_COMMAND: "/tmp/mode-test" },
      }),
    ).toEqual({
      kind: "internal-cli",
      command: "/tmp/mode-test",
      args: ["__internal-search"],
    });
  });
});
