package main

import (
	"encoding/json"
	"sort"
	"strconv"
	"strings"
)

const (
	lanDiscoveryPort            = 45879
	lanDiscoveryProbe           = "mode-lan-discover-v1"
	lanDiscoveryProtocolVersion = 1
)

// lanAnnouncement 是 Mode 桌面端 LAN 发现应答的 JSON 形状（packages/shared/src/lanAccess.ts）。
type lanAnnouncement struct {
	Magic           string `json:"magic"`
	ProtocolVersion int    `json:"protocolVersion"`
	ServerID        string `json:"serverId"`
	Name            string `json:"name"`
	Version         string `json:"version"`
	Port            int    `json:"port"`
	RequiresPairing bool   `json:"requiresPairing"`
	Platform        string `json:"platform"`
}

type desktopPeer struct {
	Host            string
	Port            int
	ServerID        string
	Name            string
	Version         string
	RequiresPairing bool
}

func parseLanAnnouncement(data []byte) (lanAnnouncement, bool) {
	var announcement lanAnnouncement
	if err := json.Unmarshal(data, &announcement); err != nil {
		return lanAnnouncement{}, false
	}
	if announcement.Magic != "mode-lan" ||
		announcement.ProtocolVersion != lanDiscoveryProtocolVersion ||
		strings.TrimSpace(announcement.ServerID) == "" ||
		announcement.Port <= 0 ||
		announcement.Port > 65535 {
		return lanAnnouncement{}, false
	}
	return announcement, true
}

// desktopPeerRank 与桌面端地址排序一致：物理局域网优先，Tailscale/CGNAT 其次。
func desktopPeerRank(host string) int {
	parts := strings.Split(host, ".")
	if len(parts) != 4 {
		return 6
	}
	first, err := strconv.Atoi(parts[0])
	if err != nil || first < 0 || first > 255 {
		return 6
	}
	second, err := strconv.Atoi(parts[1])
	if err != nil || second < 0 || second > 255 {
		return 6
	}
	switch {
	case first == 192 && second == 168:
		return 0
	case first == 10:
		return 1
	case first == 172 && second >= 16 && second <= 31:
		return 2
	case first == 100 && second >= 64 && second <= 127:
		return 3
	case first == 169 && second == 254:
		return 9
	default:
		return 5
	}
}

func sortDesktopPeers(peers []desktopPeer) {
	sort.SliceStable(peers, func(i, j int) bool {
		leftRank := desktopPeerRank(peers[i].Host)
		rightRank := desktopPeerRank(peers[j].Host)
		if leftRank != rightRank {
			return leftRank < rightRank
		}
		return peers[i].Host < peers[j].Host
	})
}
