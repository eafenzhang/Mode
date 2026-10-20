package main

import (
	"net"
	"os"
	"sort"
	"strconv"
	"strings"
)

// localIPv4Addresses returns reachable non-loopback IPv4 addresses in a
// deterministic preference order: home/office LANs first, then Tailscale-style
// CGNAT, then everything else. Link-local addresses are last.
func localIPv4Addresses() []string {
	var values []string
	if addrs, err := net.InterfaceAddrs(); err == nil {
		for _, addr := range addrs {
			ipNet, ok := addr.(*net.IPNet)
			if !ok || ipNet.IP == nil {
				continue
			}
			ip4 := ipNet.IP.To4()
			if ip4 == nil || ip4.IsLoopback() {
				continue
			}
			values = append(values, ip4.String())
		}
	}
	values = uniqueStrings(values)
	sort.Slice(values, func(i, j int) bool {
		leftRank := addressRank(values[i])
		rightRank := addressRank(values[j])
		if leftRank != rightRank {
			return leftRank < rightRank
		}
		return values[i] < values[j]
	})
	if len(values) == 0 {
		return []string{"127.0.0.1"}
	}
	return values
}

func addressRank(value string) int {
	ip := net.ParseIP(value).To4()
	if ip == nil {
		return 5
	}
	switch {
	case ip[0] == 192 && ip[1] == 168:
		return 0
	case ip[0] == 10:
		return 1
	case ip[0] == 172 && ip[1] >= 16 && ip[1] <= 31:
		return 2
	case ip[0] == 100 && ip[1] >= 64 && ip[1] <= 127:
		return 3
	case ip[0] == 169 && ip[1] == 254:
		return 9
	default:
		return 5
	}
}

func primaryPairAddress() string {
	values := localIPv4Addresses()
	for _, value := range values {
		if value != "127.0.0.1" {
			return value
		}
	}
	if len(values) > 0 {
		return values[0]
	}
	return "127.0.0.1"
}

func hostnameOr(fallback string) string {
	hostname, err := os.Hostname()
	if err != nil || strings.TrimSpace(hostname) == "" {
		return fallback
	}
	return hostname
}

func formatAddressList(values []string) string {
	if len(values) == 0 {
		return "127.0.0.1"
	}
	return strings.Join(values, ", ")
}

func atoiOr(value string, fallback int) int {
	parsed, err := strconv.Atoi(value)
	if err != nil {
		return fallback
	}
	return parsed
}
