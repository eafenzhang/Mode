#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
repo_root="$(cd ../.. && pwd)"
version="${MODE_AGENT_VERSION:-0.1.0}"
out_dir="${MODE_AGENT_OUT_DIR:-dist}"
mkdir -p "$out_dir"

xp_go="${MODE_GO_XP:-$repo_root/.tools/go/bin/go.exe}"
if [[ ! -x "$xp_go" ]]; then
  xp_go="go"
fi
modern_go="${MODE_GO_MODERN:-$repo_root/.tools/go-modern/go/bin/go.exe}"
if [[ ! -x "$modern_go" ]]; then
  modern_go="go"
fi

build() {
  local go="$1" goos="$2" goarch="$3" output="$4" module_mode="$5"
  echo "build $goos/$goarch -> $output"
  GO111MODULE="$module_mode" GOOS="$goos" GOARCH="$goarch" CGO_ENABLED=0 \
    "$go" build -ldflags "-s -w -X main.version=$version" -o "$out_dir/$output" .
}

set_icon() {
  local path="$1" description="$2"
  local rcedit="$repo_root/node_modules/electron-winstaller/vendor/rcedit.exe"
  if [[ ! -f "$rcedit" ]]; then
    echo "rcedit not found; skipping icon for $path" >&2
    return
  fi
  if [[ ! -x "$rcedit" ]]; then
    echo "rcedit cannot run on this host; skipping icon for $path" >&2
    return
  fi
  "$rcedit" "$path" --set-icon "$PWD/assets/mode-agent.ico" >/dev/null 2>&1 || \
    echo "rcedit failed for $path" >&2
}

echo "XP toolchain: $xp_go"
"$xp_go" version || {
  echo "XP Go toolchain is not runnable; install Go 1.10.8 or set MODE_GO_XP" >&2
  exit 1
}
build "$xp_go" windows 386 mode-agent-xp-x86.exe off
set_icon "$out_dir/mode-agent-xp-x86.exe" "Mode Agent remote runtime"
echo "WARNING: mode-agent-xp-x86.exe embeds Go 1.10 (EOL). Use it only on a trusted LAN or VPN."

echo "Modern toolchain: $modern_go"
if "$modern_go" version >/dev/null 2>&1; then
  build "$modern_go" windows amd64 mode-agent-win-x64.exe on
  build "$modern_go" windows 386 mode-agent-win-x86.exe on
  set_icon "$out_dir/mode-agent-win-x64.exe" "Mode Agent remote runtime"
  set_icon "$out_dir/mode-agent-win-x86.exe" "Mode Agent remote runtime"
  build "$modern_go" linux amd64 mode-remote-linux-amd64 on
  build "$modern_go" linux arm64 mode-remote-linux-arm64 on
  build "$modern_go" darwin amd64 mode-remote-darwin-amd64 on
  build "$modern_go" darwin arm64 mode-remote-darwin-arm64 on
  if [[ "${MODE_AGENT_SKIP_TESTS:-0}" != "1" ]]; then
    "$modern_go" test .
  fi
else
  echo "modern Go toolchain not available; skipped modern Windows and client builds" >&2
fi

if [[ ! -f "$out_dir/mode-agent-win-x64.exe" ]]; then
  echo "mode-agent-win-x64.exe missing; cannot build the single installer" >&2
  exit 1
fi
bootstrap="$out_dir/mode-agent-setup-bootstrap.exe"
GO111MODULE=off GOOS=windows GOARCH=386 CGO_ENABLED=0 \
  "$xp_go" build -ldflags "-s -w" -o "$bootstrap" ./setup
set_icon "$bootstrap" "Mode Agent installer"
"$modern_go" run ./setup/pack/main.go \
  -bootstrap "$bootstrap" \
  -x86 "$out_dir/mode-agent-xp-x86.exe" \
  -x64 "$out_dir/mode-agent-win-x64.exe" \
  -out "$out_dir/ModeAgent-Setup.exe"
rm -f "$bootstrap" "$out_dir/ModeAgent-Setup-x86.exe" "$out_dir/ModeAgent-Setup-x64.exe"
ls -lh "$out_dir"
