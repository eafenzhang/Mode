# @mode/cua

Computer Use host package (Windows first phase): a broker line protocol plus a
helper child process and its Rust addon (`crates/mode-cua-ax`, UIA3 / GDI /
SendInput), with the Computer Use runtime executing in the node_repl host.
macOS surfaces remain fail-closed placeholders. Capabilities and permission
boundaries (lock screen → `permission_denied`, elevated UIPI targets →
`action_unavailable`, no TCC-style prompt on Windows, socket-path credential
for helper connections) are documented in
[the Windows runtime spec](../../docs/specs/computer-use-windows-runtime.md).

## Development

```bash
pnpm build:cua-helper   # builds dist-cua-helper and stages it into bundled-tools
# then start desktop dev against this package's outputs:
# PowerShell: $env:MODE_CUA_DEV_ROOT="$(Get-Location)\packages\mode-cua"; pnpm dev:desktop
# Git Bash : MODE_CUA_DEV_ROOT="$(pwd)/packages/mode-cua" pnpm dev:desktop
```

`MODE_CUA_DEV_ROOT` points a desktop dev run at this package's build outputs
(`dist-cua-helper/entry.cjs` and `dist-cua-helper/cua_ax.node` per the
`modeCuaRuntime` contract in `package.json`).

## Tests

```bash
pnpm --filter @mode/cua test
CUA_INTEGRATION=1 pnpm --filter @mode/cua run test:integration
```

The integration test needs `pnpm build:cua-helper` artifacts and skips itself
otherwise.

License: Apache-2.0.
