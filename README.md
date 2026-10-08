# Mode

<div align="center">
  <img src="packages/desktop/build/icons/512x512.png" alt="Mode" width="96" height="96" />
  <p><strong>An AI coding workspace for desktop, browser and terminal — forked from ZCode, maintained independently.</strong></p>
</div>

<p align="center">
  <a href="README.zh-CN.md">简体中文</a> | English
</p>

<p align="center">
  <a href="https://github.com/eafenzhang/Mode/releases"><img src="https://img.shields.io/github/v/release/eafenzhang/Mode?label=release" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
</p>

Mode keeps the product itself — an agent that plans, edits, runs and verifies code with you — and rebuilds it from the public source with monitoring and telemetry removed and the vendor platform retired. It continues the ZCodium audit fork of [zai-org/ZCode](https://github.com/zai-org/ZCode); the lineage is recorded in [NOTICE.md](NOTICE.md) and the git history.

## What this fork is about

- **No monitoring, no telemetry**: the client monitoring SDK, usage and network reporting, crash collection, resource sampling and UI instrumentation are gone, with regression checks that keep them out.
- **Official platform retired**: sign-in, coding plans, official MCP credentials, feedback upload and the official plugin marketplace no longer connect anywhere. There is no switch that turns them back on.
- **Offline plugin catalog**: the "Public" segment of the plugin store ships the official plugin list inside the app (icons and Chinese descriptions included), so it browses without touching a CDN; the "Personal" segment is whatever directories you add yourself, such as the Claude Code plugin directory.
- **Built for daily use**: IM bots, LAN remote connections and silent updates, described below.

## Highlights

- **One agent, three interfaces**: the Electron desktop app, the browser workspace and the terminal TUI share one agent runtime and the same sessions. Remote workspaces work over SSH, WSL, Docker, or a **LAN peer** running Mode on another machine in the same network.
- **Plans, edits, runs, verifies**: file changes arrive as diffs, terminal commands carry their context, and the agent checks its own work by running tests. A bundled browser plugin drives a real browser when a task needs one.
- **Asks before it touches your project**: every edit, command and tool call can require approval — allow once, always in this project, or full access.
- **IM bots**: drive a workspace from 微信 / 企业微信 / 飞书（中国）/ 钉钉. Each conversation keeps its own session binding (per person in private chat, per group in group chat), replies stream back into the chat, group chats can stay silent (WeCom only answers when mentioned), and an optional heartbeat posts a summary on a schedule.
- **LAN remote connection**: another machine in the same network running Mode is discovered over UDP broadcast, paired with a one-time 6-digit code, and then used as a remote host — pick a workspace on it and chat as if it were local.
- **Multi-agent collaboration**: sub-agents, dynamic workflows, skills and scheduled automations.
- **Plugins, skills and MCP**: a built-in plugin set plus a marketplace with an offline official catalog; MCP servers are configured per user or workspace.
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

Prerequisites: Git, Node.js **24.14.0** and pnpm **10.33.2** — [mise.toml](mise.toml) is the source of truth for tool versions. Run everything from the repository root.

```bash
pnpm bootstrap                 # install dependencies and prepare local desktop runtime assets
pnpm dev:desktop               # Electron desktop app (production config; use dev:desktop:test for the test env)
pnpm dev:web                   # browser workspace
pnpm --filter @mode/cli dev   # agent CLI
```

Set `MODE_DATA_BASE_DIR` to develop against an isolated data directory instead of your real one.

Useful checks before committing:

```bash
pnpm typecheck                                              # TypeScript project references
pnpm lint                                                   # oxlint
pnpm --filter @mode/services test                          # service and contract tests
node --test packages/desktop/tests/*.test.mjs               # desktop node tests
pnpm architecture:check -- --changed                        # dependency direction policy
```

To produce a desktop installer locally (artifacts land in `packages/desktop/dist`):

```bash
pnpm bundle:desktop -- --os win --arch x64
```

## Release automation

Pushing to `main` builds and publishes a new version automatically: `.github/workflows/release.yml` takes the latest stable tag, bumps its patch (for example `v0.0.1` → `v0.0.2`), runs the `verify` gate (typecheck, lint, service and desktop tests), builds the desktop clients for Windows x64, macOS Apple Silicon and Linux x64, uploads everything into a **draft** release, and publishes it only after every artifact is in place — a failed build leaves the release as a draft, so download pages never resolve to a half-built version. Put `[skip release]` in the commit message to push without releasing, or trigger the workflow manually to pick an explicit version or a pre-release.

## Repository layout

| Path                                                 | Contents                                                                   |
| ---------------------------------------------------- | -------------------------------------------------------------------------- |
| `packages/desktop`                                   | Electron main process, host and renderer; LAN access server and discovery  |
| `packages/ui`                                        | Shared React components, hooks and Zustand stores (settings, plugin store) |
| `packages/services`                                  | Business services (agent sessions, bots, plugins, remote connections)      |
| `packages/server`, `packages/web`                    | Browser workspace server and client                                        |
| `packages/client`, `packages/rpc`, `packages/shared` | Agent client SDK, RPC framework, shared contracts                          |
| `apps/zcode-cli`                                     | Agent CLI and runtime (also embedded by the desktop app)                   |
| `harness/lan`                                        | How to try the LAN remote connection between two machines                  |

## License and provenance

First-party code in this repository is MIT licensed ([LICENSE](LICENSE)); the upstream ZCode source it forks is Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)). Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md); what the app may do on your machine — file access, commands, hooks, browser automation, background tasks — is described in [NOTICE.md](NOTICE.md) (also available as [NOTICE.zh-CN.md](NOTICE.zh-CN.md)).

The telemetry removals that started this fork are documented, with their limits, in the [desktop](packages/desktop/specs/telemetry-removal-report.md), [CLI](apps/zcode-cli/specs/telemetry-removal-report.md) and [UI](packages/ui/specs/telemetry-removal-report.md) reports.

## Feedback

Bugs, questions and feature requests: [open an issue](https://github.com/eafenzhang/Mode/issues). Please check the existing issues first, and include the version shown in **Settings → About**.
