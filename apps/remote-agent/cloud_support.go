package main

import (
	"net"
	"strconv"
	"strings"
)

func localAgentBaseURL(cfg *Config) string {
	host := strings.TrimSpace(cfg.Bind)
	if host == "" || host == "0.0.0.0" || host == "::" || host == "[::]" {
		host = "127.0.0.1"
	}
	return "http://" + net.JoinHostPort(host, strconv.Itoa(cfg.Port))
}
func configPathOf(cfg *Config) string {
	if cfg.configPath != "" {
		return cfg.configPath
	}
	return resolveConfigPath("")
}

type cloudLoginResult struct {
	OK      bool   `json:"ok"`
	Account string `json:"account,omitempty"`
	Error   string `json:"error,omitempty"`
}
