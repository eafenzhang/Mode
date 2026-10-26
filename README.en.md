# Mode

<div align="center">
  <img src="apps/desktop-tauri/src-tauri/icons/icon.png" alt="Mode" width="96" height="96" />
  <p><strong>An AI coding workspace for desktop, browser and terminal, with its core rewritten in Rust.</strong></p>
</div>

<p align="center">
  <a href="README.md">简体中文</a> | English | <a href="https://mymode.cloud/">Website</a>
</p>

<p align="center">
  <a href="https://github.com/eafenzhang/Mode/releases"><img src="https://img.shields.io/github/v/release/eafenzhang/Mode?label=release" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
</p>

Mode is an agent that plans, edits, runs and verifies code with you. The desktop app is rebuilt on Rust + Tauri v2: window and platform capabilities, the protocol source of truth, updater / tray / deep links and Host sidecar supervision all live in the Rust workspace (`crates/`); the Rust rewrite of the agent session runtime and tool execution is in `crates/agent-core` and `crates/agent-tools`. The UI reuses shared React components, and ecosystem capabilities such as plugins and IM bots are carried by the bundled Host sidecar, moving into Rust step by step per the roadmap.

## Highlights

- **One agent, three interfaces**: the Tauri desktop app, the browser workspace and the terminal TUI share one agent runtime and the same sessions. Remote workspaces work over SSH, WSL, Docker, or a **LAN peer** running Mode on another machine in the same network.
- **Plans, edits, runs, verifies**: file changes arrive as diffs, terminal commands carry their context, and the agent checks its own work by running tests. A bundled browser plugin drives a real browser when a task needs one.
- **Asks before it touches your project**: every edit, command and tool call can require approval — allow once, always in this project, or full access.
- **IM bots**: drive a workspace from WeChat / WeCom / Feishu (China) / DingTalk. Each conversation keeps its own session binding (per person in private chat, per group in group chat), replies stream back into the chat, group chats can stay silent (WeCom only answers when mentioned), and an optional heartbeat posts a summary on a schedule.
- **LAN remote connection**: another machine in the same network running Mode is discovered over UDP broadcast, paired with a one-time 6-digit code, and then used as a remote host — pick a workspace on it and chat as if it were local.
- **Remote controlled agent**: a Windows machine (XP through 11) can run the single-file `mode-agent`, which takes commands over standard MCP Streamable HTTP — commands and background jobs, files, processes, screenshots and keyboard/mouse control. See [apps/remote-agent](apps/remote-agent/README.md).
- **Multi-agent collaboration**: sub-agents, dynamic workflows, skills and scheduled automations.
- **Plugins, skills and MCP**: a built-in plugin set plus a marketplace; the "Public" segment ships an offline catalog inside the app (icons and Chinese descriptions included) and browses without touching a CDN, while the "Personal" segment holds directories you register yourself, such as a Claude Code plugin directory. MCP servers are configured per user or workspace.
- **Bring your own model**: built-in presets for DeepSeek, OpenAI, Anthropic, Moonshot Kimi, MiniMax, Z.AI (GLM), Alibaba, xAI, Xiaomi MiMo and OpenRouter, plus fully custom endpoints (Chat Completions, Responses, Anthropic Messages).
- **Silent updates**: the app checks this repository's GitHub Releases itself, honors your proxy settings, and installs on quit when you let it.

## Download and install

Installers are attached to the [Releases](https://github.com/eafenzhang/Mode/releases) page. Builds are **not code-signed**, so each system blocks the first launch once — that is expected, and the download can be verified against `sha256.txt` on the same release page (`certutil -hashfile <file> SHA256` on Windows, `shasum -a 256 <file>` on macOS, `sha256sum <file>` on Linux).

| Platform            | Asset                                  | First launch                                                                              |
| ------------------- | -------------------------------------- | ----------------------------------------------------------------------------------------- |
| Windows x64         | `Mode-<version>-win-x64.exe`           | SmartScreen warns: **More info** → **Run anyway**                                         |
| macOS Apple Silicon | `Mode-<version>-mac-arm64.dmg`         | `sudo /usr/bin/xattr -rd com.apple.quarantine "/Applications/Mode.app" && open -a "Mode"` |
| Linux x86_64        | `Mode-<version>-linux-x86_64.AppImage` | `chmod +x` the file, then run it                                                          |

Installed desktop clients update themselves from this repository's releases.

Remote workspaces over SSH, WSL or Docker reuse a runtime that already exists on the host; this repository does not publish the prebuilt runtime bundles. To provision a host that has none, build them locally with `pnpm prepare:remote-assets` and point `MODE_REMOTE_ASSET_CDN_BASE_URL` at wherever you host them.

## Build and run from source

Prerequisites: Git, Node.js **24.14.0** and pnpm **10.33.2** ([mise.toml](mise.toml) is the source of truth for tool versions), plus a Rust stable toolchain ([rust-toolchain.toml](rust-toolchain.toml), with clippy and rustfmt). Run everything from the repository root.

```bash
pnpm bootstrap                 # install dependencies and build workspace packages
pnpm dev:desktop               # Tauri desktop app (Rust shell + shared renderer)
pnpm dev:web                   # browser workspace
pnpm --filter @mode/cli dev   # agent CLI
```

Set `MODE_DATA_BASE_DIR` to develop against an isolated data directory instead of your real one; `mise run dev` has this isolation built in.

Useful checks before committing:

```bash
pnpm typecheck                            # TypeScript project references
pnpm lint                                 # oxlint
cargo test --workspace                    # Rust workspace tests
cargo clippy --all-targets -- -D warnings # Rust static checks
pnpm architecture:check -- --changed      # dependency direction policy
```

On Windows without VS Build Tools, run Rust commands through `mise run rust -- test --workspace` (it assembles a gnu toolchain automatically; CI and regular environments use `cargo` directly).

To produce a desktop installer locally (artifacts land in `dist/tauri`):

```bash
pnpm --filter @mode/desktop-tauri run tauri:build
```

## Release automation

Pushing to `main` builds and publishes a new version automatically: `.github/workflows/release.yml` takes the latest stable tag, bumps its patch (for example `v0.0.1` → `v0.0.2`), runs the `verify` gate (typecheck, lint, service and desktop tests), builds the desktop clients for Windows x64, macOS Apple Silicon and Linux x64, uploads everything into a **draft** release, and publishes it only after every artifact is in place — a failed build leaves the release as a draft, so download pages never resolve to a half-built version. Put `[skip release]` in the commit message to push without releasing, or trigger the workflow manually to pick an explicit version or a pre-release.

## Repository layout

| Path                                                 | Contents                                                                                                                |
| ---------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `crates/`                                            | Rust workspace: protocol source of truth (`protocol`), session runtime (`agent-core`, `agent-tools`), platform command surface (`platform`), Host sidecar supervision (`sidecar-host`), plugin host, updater, tray, deep links, and more |
| `apps/desktop-tauri`                                 | Tauri v2 shell (Rust command surface, packaging)                                                                         |
| `apps/remote-agent`                                  | Windows remote controlled agent `mode-agent` (Go, XP through 11)                                                         |
| `apps/mode-cli`                                      | Agent CLI and runtime                                                                                                    |
| `packages/desktop`                                   | Product renderer and Host sidecar sources; LAN access server and discovery                                               |
| `packages/ui`                                        | Shared React components, hooks and Zustand stores (settings, plugin store)                                               |
| `packages/services`                                  | Business services (agent sessions, bots, plugins, remote connections)                                                    |
| `packages/web`, `packages/server`                    | Browser workspace client and server                                                                                      |
| `packages/client`, `packages/rpc`, `packages/shared` | Agent client SDK, RPC framework, shared contracts                                                                        |
| `docs/specs`                                         | Product rules, protocols and acceptance scenarios                                                                        |

## License and notices

First-party code in this repository is MIT licensed ([LICENSE](LICENSE)); code derived from Apache-2.0 projects keeps its original license ([LICENSE-APACHE](LICENSE-APACHE)). Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md); what the app may do on your machine — file access, commands, hooks, browser automation, background tasks — is described in [NOTICE.md](NOTICE.md) (also available as [NOTICE.zh-CN.md](NOTICE.zh-CN.md)).

## Feedback

Bugs, questions and feature requests: [open an issue](https://github.com/eafenzhang/Mode/issues). Please check the existing issues first, and include the version shown in **Settings → About**.
