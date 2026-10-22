//go:build windows
// +build windows

package main

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"net"
	"os/exec"
	"sort"
	"strings"
	"time"
)

// discoverDesktops 使用桌面端已有的 LAN 发现协议查找候选桌面：
// UDP 45879 广播/定向广播 + Tailscale 100.x 单播；应答不含令牌。
func discoverDesktops(timeout time.Duration) []desktopPeer {
	conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero, Port: 0})
	if err != nil {
		return nil
	}
	defer conn.Close()
	_ = conn.SetReadDeadline(time.Now().Add(timeout))
	payload := []byte(lanDiscoveryProbe)
	for _, target := range probeTargets() {
		ip := net.ParseIP(target)
		if ip == nil {
			continue
		}
		_, _ = conn.WriteToUDP(payload, &net.UDPAddr{IP: ip, Port: lanDiscoveryPort})
	}

	peersByKey := map[string]desktopPeer{}
	buffer := make([]byte, 4096)
	for {
		length, address, readErr := conn.ReadFromUDP(buffer)
		if readErr != nil {
			break
		}
		announcement, ok := parseLanAnnouncement(buffer[:length])
		if !ok {
			continue
		}
		host := address.IP.String()
		key := fmt.Sprintf("%s@%s:%d", announcement.ServerID, host, announcement.Port)
		peersByKey[key] = desktopPeer{
			Host:            host,
			Port:            announcement.Port,
			ServerID:        announcement.ServerID,
			Name:            announcement.Name,
			Version:         announcement.Version,
			RequiresPairing: announcement.RequiresPairing,
		}
	}
	peers := make([]desktopPeer, 0, len(peersByKey))
	for _, peer := range peersByKey {
		peers = append(peers, peer)
	}
	sortDesktopPeers(peers)
	return peers
}

func probeTargets() []string {
	targets := map[string]bool{"255.255.255.255": true}
	interfaces, err := net.Interfaces()
	if err == nil {
		for _, iface := range interfaces {
			if iface.Flags&net.FlagUp == 0 {
				continue
			}
			addrs, addrErr := iface.Addrs()
			if addrErr != nil {
				continue
			}
			for _, addr := range addrs {
				ipnet, ok := addr.(*net.IPNet)
				if !ok {
					continue
				}
				ip4 := ipnet.IP.To4()
				if ip4 == nil || len(ipnet.Mask) != 4 {
					continue
				}
				address := binary.BigEndian.Uint32(ip4)
				mask := binary.BigEndian.Uint32(ipnet.Mask)
				broadcast := address | ^mask
				targets[net.IPv4(byte(broadcast>>24), byte(broadcast>>16), byte(broadcast>>8), byte(broadcast)).String()] = true
			}
		}
	}
	for _, ip := range tailscaleIPs() {
		targets[ip] = true
	}
	result := make([]string, 0, len(targets))
	for target := range targets {
		result = append(result, target)
	}
	sort.Strings(result)
	return result
}

// tailscaleIPs 读取本机 tailnet 的 IPv4 地址；CLI 缺席/超时/解析失败一律静默降级。
func tailscaleIPs() []string {
	candidates := []string{
		"tailscale",
		`C:\Program Files\Tailscale\tailscale.exe`,
	}
	for _, candidate := range candidates {
		ctx, cancel := context.WithTimeout(context.Background(), 800*time.Millisecond)
		command := exec.CommandContext(ctx, candidate, "status", "--json")
		command.SysProcAttr = newSysProcAttr()
		output, err := command.Output()
		cancel()
		if err != nil {
			continue
		}
		var status struct {
			Self struct {
				TailscaleIPs []string `json:"TailscaleIPs"`
			} `json:"Self"`
			Peer map[string]struct {
				TailscaleIPs []string `json:"TailscaleIPs"`
			} `json:"Peer"`
		}
		if err := json.Unmarshal(output, &status); err != nil {
			continue
		}
		seen := map[string]bool{}
		collect := func(values []string) {
			for _, value := range values {
				ip := net.ParseIP(strings.TrimSpace(value))
				if ip == nil || ip.To4() == nil {
					continue
				}
				seen[ip.To4().String()] = true
			}
		}
		collect(status.Self.TailscaleIPs)
		for _, peer := range status.Peer {
			collect(peer.TailscaleIPs)
		}
		result := make([]string, 0, len(seen))
		for ip := range seen {
			result = append(result, ip)
		}
		sort.Strings(result)
		return result
	}
	return nil
}
