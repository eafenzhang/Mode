//go:build windows
// +build windows

package main

import (
	"context"
	"crypto/tls"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"time"
)

func cmdServe(args []string) int {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	bind := fs.String("bind", "", "override listen address")
	port := fs.Int("port", -1, "override listen port (0 = ephemeral)")
	uiFlag := fs.Bool("ui", false, "start the tray panel (default from config)")
	noUI := fs.Bool("no-ui", false, "disable the tray panel")
	openPanel := fs.Bool("open-panel", false, "open the panel window on start")
	pairOnStart := fs.Bool("pair-on-start", false, "show the desktop pairing prompt after startup")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	if *bind != "" {
		cfg.Bind = *bind
	}
	if *port >= 0 {
		cfg.Port = *port
	}
	server, err := newAgentServer(cfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "initialize server: %v\n", err)
		return 1
	}
	defer server.close()
	tlsConfig, err := cfg.tlsConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	address := net.JoinHostPort(cfg.Bind, strconv.Itoa(cfg.Port))
	listener, err := net.Listen("tcp", address)
	if err != nil {
		fmt.Fprintf(os.Stderr, "listen on %s: %v\n", address, err)
		return 1
	}
	fmt.Printf("listening=%s\n", listener.Addr().String())
	scheme := "http"
	if tlsConfig != nil {
		scheme = "https"
	}
	fmt.Fprintf(os.Stderr, "%s %s (%s) listening on %s://%s\n", agentName, version, goVersion, scheme, listener.Addr().String())

	// 面板关闭但托盘开启时仍启动 UI：用户需要托盘里的「设置与配对」重新打开面板。
	uiEnabled := cfg.Panel.Enabled || cfg.Panel.ShowTray || *uiFlag
	if *noUI {
		uiEnabled = false
	}
	if *openPanel {
		uiEnabled = true
	}
	var ui *agentUIHandle
	if uiEnabled {
		ui, err = startAgentUI(cfg, server.panel, server.requestShutdown)
		if err != nil {
			fmt.Fprintf(os.Stderr, "panel UI disabled: %v\n", err)
			ui = nil
		}
	}
	if ui != nil {
		defer ui.stop()
		if *openPanel {
			ui.show()
		}
	}

	httpServer := &http.Server{
		Handler:           server,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       120 * time.Second,
	}
	if *pairOnStart {
		// 等 HTTP 监听就绪后再弹配对页：桌面端登记时会探测 /health。
		go func() {
			time.Sleep(700 * time.Millisecond)
			promptPortablePairing(cfg)
		}()
	}
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt)
	go func() {
		select {
		case <-signals:
		case <-server.shutdownCh:
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		httpServer.Shutdown(ctx)
	}()
	if tlsConfig != nil {
		listener = tls.NewListener(listener, tlsConfig)
	}
	if err := httpServer.Serve(listener); err != nil && err != http.ErrServerClosed {
		fmt.Fprintf(os.Stderr, "serve failed: %v\n", err)
		return 1
	}
	return 0
}

func promptPortablePairing(cfg *Config) {
	if cfg == nil || cfg.configPath == "" {
		return
	}
	dir := filepath.Dir(cfg.configPath)
	paths := installPaths{
		Dir:    dir,
		Exe:    filepath.Join(dir, "mode-agent.exe"),
		Config: cfg.configPath,
		Info:   filepath.Join(dir, "install-info.txt"),
	}
	promptAndEnroll(paths, cfg)
}
