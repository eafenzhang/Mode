package main

import (
	"fmt"
	"os"
	"runtime"
)

func main() {
	if len(os.Args) < 2 {
		if runtime.GOOS == "windows" {
			os.Exit(cmdSetup(nil))
		}
		usage()
		os.Exit(2)
	}
	switch os.Args[1] {
	case "init":
		os.Exit(cmdInit(os.Args[2:]))
	case "install":
		os.Exit(cmdInstall(os.Args[2:]))
	case "setup":
		os.Exit(cmdSetup(os.Args[2:]))
	case "uninstall":
		os.Exit(cmdUninstall(os.Args[2:]))
	case "pair":
		os.Exit(cmdPair(os.Args[2:]))
	case "enroll":
		os.Exit(cmdEnroll(os.Args[2:]))
	case "serve":
		os.Exit(cmdServe(os.Args[2:]))
	case "token":
		os.Exit(cmdToken(os.Args[2:]))
	case "remote":
		os.Exit(cmdRemote(os.Args[2:]))
	case "install-autostart":
		os.Exit(cmdInstallAutostart(os.Args[2:]))
	case "uninstall-autostart":
		os.Exit(cmdUninstallAutostart(os.Args[2:]))
	case "version", "--version", "-version":
		os.Exit(cmdVersion(os.Args[2:]))
	case "help", "--help", "-h":
		usage()
		os.Exit(0)
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n\n", os.Args[1])
		usage()
		os.Exit(2)
	}
}

func usage() {
	fmt.Fprint(os.Stderr, `mode-agent - remote Windows runtime for Mode

Usage:
  mode-agent install [--dir PATH] [--port N] [--no-firewall] [--no-autostart] [--no-start] [--quiet]
  mode-agent setup [--yes] [--dir PATH]
  mode-agent uninstall [--dir PATH] [--purge] [--yes]
  mode-agent enroll [--config PATH] [--uri URI | --desktop HOST:PORT --code CODE] [--no-dialog]
  mode-agent pair [--config PATH] [--no-dialog]
  mode-agent init [--config PATH] [--lan] [--tls] [--port N] [--force]
  mode-agent serve [--config PATH] [--bind ADDR] [--port N]
  mode-agent token [--config PATH]
  mode-agent install-autostart [--config PATH] [--minimized]
  mode-agent uninstall-autostart
  mode-agent remote <info|exec|job-start|job-status|job-write|job-cancel|job-list|fs-list|fs-read|fs-write|fs-stat|fs-remove|ps|kill|screen|mouse|key|windows|focus|db-drivers|db-query|mcp-tools|raw> [flags]
  mode-agent version

Security:
  The token grants the configured remote capabilities. Keep it secret and use
  the agent only on a trusted LAN or VPN unless TLS is configured.
`)
}

func osName() string {
	return runtime.GOOS
}

func archName() string {
	return runtime.GOARCH
}
