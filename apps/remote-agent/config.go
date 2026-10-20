package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"flag"
	"fmt"
	"io/ioutil"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

const (
	defaultBind = "127.0.0.1"
	defaultPort = 47821
)

type Config struct {
	Bind                string            `json:"bind"`
	Port                int               `json:"port"`
	AgentID             string            `json:"agentId,omitempty"`
	Token               string            `json:"token"`
	TLSCertFile         string            `json:"tlsCertFile,omitempty"`
	TLSKeyFile          string            `json:"tlsKeyFile,omitempty"`
	DataDir             string            `json:"dataDir,omitempty"`
	AllowedRoots        []string          `json:"allowedRoots,omitempty"`
	AllowCIDRs          []string          `json:"allowCidrs,omitempty"`
	AllowShell          bool              `json:"allowShell"`
	AllowWrite          bool              `json:"allowWrite"`
	AllowDelete         bool              `json:"allowDelete"`
	AllowDesktopControl bool              `json:"allowDesktopControl"`
	AllowDB             bool              `json:"allowDb"`
	AllowDBWrite        bool              `json:"allowDbWrite"`
	DSNs                map[string]string `json:"dsns,omitempty"`

	configPath string
}

func defaultConfig() *Config {
	return &Config{
		Bind:       defaultBind,
		Port:       defaultPort,
		AllowShell: true,
		AllowDB:    true,
		DSNs:       map[string]string{},
	}
}

func configDir() string {
	if dir := strings.TrimSpace(os.Getenv("APPDATA")); dir != "" {
		return filepath.Join(dir, "ModeAgent")
	}
	if profile := strings.TrimSpace(os.Getenv("USERPROFILE")); profile != "" {
		return filepath.Join(profile, "ModeAgent")
	}
	return filepath.Join(os.TempDir(), "ModeAgent")
}

func writableDir(dir string) bool {
	if err := os.MkdirAll(dir, 0700); err != nil {
		return false
	}
	f, err := ioutil.TempFile(dir, ".mode-agent-write-")
	if err != nil {
		return false
	}
	name := f.Name()
	f.Close()
	os.Remove(name)
	return true
}

func resolveConfigPath(explicit string) string {
	if strings.TrimSpace(explicit) != "" {
		abs, err := filepath.Abs(explicit)
		if err == nil {
			return abs
		}
		return explicit
	}
	exe, err := os.Executable()
	if err == nil {
		dir := filepath.Dir(exe)
		if writableDir(dir) {
			return filepath.Join(dir, "mode-agent.json")
		}
	}
	dir := configDir()
	os.MkdirAll(dir, 0700)
	return filepath.Join(dir, "mode-agent.json")
}

func loadConfig(path string) (*Config, error) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read config %s: %v", path, err)
	}
	cfg := defaultConfig()
	if err := json.Unmarshal(data, cfg); err != nil {
		return nil, fmt.Errorf("parse config %s: %v", path, err)
	}
	cfg.configPath = path
	if strings.TrimSpace(cfg.Bind) == "" {
		return nil, fmt.Errorf("config bind must not be empty")
	}
	if cfg.Port < 1 || cfg.Port > 65535 {
		return nil, fmt.Errorf("config port must be 1..65535")
	}
	if strings.TrimSpace(cfg.Token) == "" {
		return nil, fmt.Errorf("config token is empty; run `mode-agent init` first")
	}
	if (cfg.TLSCertFile == "") != (cfg.TLSKeyFile == "") {
		return nil, fmt.Errorf("tlsCertFile and tlsKeyFile must be configured together")
	}
	if cfg.DataDir == "" {
		cfg.DataDir = filepath.Dir(path)
	}
	if abs, err := filepath.Abs(cfg.DataDir); err == nil {
		cfg.DataDir = abs
	}
	if cfg.DSNs == nil {
		cfg.DSNs = map[string]string{}
	}
	if strings.TrimSpace(cfg.AgentID) == "" {
		if id, idErr := randomHex(16); idErr == nil {
			cfg.AgentID = id
			_ = saveConfig(cfg, path)
		}
	}
	for i, root := range cfg.AllowedRoots {
		if strings.TrimSpace(root) == "" {
			continue
		}
		if abs, err := filepath.Abs(root); err == nil {
			cfg.AllowedRoots[i] = filepath.Clean(abs)
		} else {
			cfg.AllowedRoots[i] = filepath.Clean(root)
		}
	}
	return cfg, nil
}

func saveConfig(cfg *Config, path string) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	return ioutil.WriteFile(path, data, 0600)
}

func randomHex(bytes int) (string, error) {
	raw := make([]byte, bytes)
	if _, err := rand.Read(raw); err != nil {
		return "", err
	}
	return hex.EncodeToString(raw), nil
}

func newToken() (string, error) {
	raw := make([]byte, 32)
	if _, err := rand.Read(raw); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(raw), nil
}

func (c *Config) tlsConfig() (*tls.Config, error) {
	if c.TLSCertFile == "" {
		return nil, nil
	}
	cert, err := tls.LoadX509KeyPair(c.TLSCertFile, c.TLSKeyFile)
	if err != nil {
		return nil, fmt.Errorf("load TLS certificate: %v", err)
	}
	return &tls.Config{
		Certificates:             []tls.Certificate{cert},
		MinVersion:               tls.VersionTLS12,
		PreferServerCipherSuites: true,
	}, nil
}

func generateSelfSignedCert(dir, hostname string) (certPath, keyPath, fingerprint string, err error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return "", "", "", err
	}
	serialRaw := make([]byte, 16)
	if _, err = rand.Read(serialRaw); err != nil {
		return "", "", "", err
	}
	serial := big.NewInt(0).SetBytes(serialRaw)
	if serial.Sign() == 0 {
		serial.SetInt64(1)
	}

	dnsNames := []string{"localhost", "mode-agent"}
	hostname = strings.TrimSpace(hostname)
	if hostname != "" {
		dnsNames = append(dnsNames, hostname, hostname+".local")
	}
	sort.Strings(dnsNames)
	dnsNames = uniqueStrings(dnsNames)

	ips := []net.IP{net.ParseIP("127.0.0.1"), net.ParseIP("::1")}
	if addrs, addrErr := net.InterfaceAddrs(); addrErr == nil {
		for _, addr := range addrs {
			if ipNet, ok := addr.(*net.IPNet); ok && ipNet.IP != nil {
				ips = append(ips, ipNet.IP)
			}
		}
	}
	ips = uniqueIPs(ips)

	template := &x509.Certificate{
		SerialNumber:          serial,
		Subject:               pkix.Name{CommonName: "mode-agent", Organization: []string{"Mode"}},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(10 * 365 * 24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
		DNSNames:              dnsNames,
		IPAddresses:           ips,
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		return "", "", "", err
	}
	keyDER, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		return "", "", "", err
	}
	certPath = filepath.Join(dir, "agent-cert.pem")
	keyPath = filepath.Join(dir, "agent-key.pem")
	certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER})
	if err = ioutil.WriteFile(certPath, certPEM, 0600); err != nil {
		return "", "", "", err
	}
	if err = ioutil.WriteFile(keyPath, keyPEM, 0600); err != nil {
		return "", "", "", err
	}
	sum := sha256.Sum256(der)
	return certPath, keyPath, hex.EncodeToString(sum[:]), nil
}

func certificateFingerprint(certPath string) (string, error) {
	data, err := ioutil.ReadFile(certPath)
	if err != nil {
		return "", err
	}
	block, _ := pem.Decode(data)
	if block == nil {
		return "", fmt.Errorf("no PEM certificate in %s", certPath)
	}
	sum := sha256.Sum256(block.Bytes)
	return hex.EncodeToString(sum[:]), nil
}

func uniqueStrings(values []string) []string {
	seen := map[string]bool{}
	out := make([]string, 0, len(values))
	for _, value := range values {
		if value == "" || seen[value] {
			continue
		}
		seen[value] = true
		out = append(out, value)
	}
	return out
}

func uniqueIPs(values []net.IP) []net.IP {
	seen := map[string]bool{}
	out := make([]net.IP, 0, len(values))
	for _, value := range values {
		if value == nil {
			continue
		}
		key := value.String()
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, value)
	}
	return out
}

func cmdInit(args []string) int {
	fs := flag.NewFlagSet("init", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	bind := fs.String("bind", "", "listen address (default 127.0.0.1, or 0.0.0.0 with --lan)")
	port := fs.Int("port", defaultPort, "listen port")
	lan := fs.Bool("lan", false, "listen on all interfaces")
	withTLS := fs.Bool("tls", false, "generate a self-signed TLS certificate")
	tokenFlag := fs.String("token", "", "use this token instead of a generated one")
	force := fs.Bool("force", false, "overwrite an existing config")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	if _, err := os.Stat(path); err == nil && !*force {
		fmt.Fprintf(os.Stderr, "config already exists: %s (use --force to overwrite)\n", path)
		return 1
	}
	cfg := defaultConfig()
	agentID, idErr := randomHex(16)
	if idErr != nil {
		fmt.Fprintf(os.Stderr, "generate agent id: %v\n", idErr)
		return 1
	}
	cfg.AgentID = agentID
	cfg.Bind = *bind
	if cfg.Bind == "" {
		if *lan {
			cfg.Bind = "0.0.0.0"
		} else {
			cfg.Bind = defaultBind
		}
	}
	cfg.Port = *port
	cfg.DataDir = filepath.Dir(path)
	cfg.configPath = path
	token := strings.TrimSpace(*tokenFlag)
	if token == "" {
		generated, err := newToken()
		if err != nil {
			fmt.Fprintf(os.Stderr, "generate token: %v\n", err)
			return 1
		}
		token = generated
	}
	cfg.Token = token
	if *withTLS {
		hostname, _ := os.Hostname()
		certPath, keyPath, fingerprint, err := generateSelfSignedCert(filepath.Dir(path), hostname)
		if err != nil {
			fmt.Fprintf(os.Stderr, "generate TLS certificate: %v\n", err)
			return 1
		}
		cfg.TLSCertFile = certPath
		cfg.TLSKeyFile = keyPath
		fmt.Printf("tlsFingerprint=SHA256:%s\n", fingerprint)
	}
	if err := saveConfig(cfg, path); err != nil {
		fmt.Fprintf(os.Stderr, "write config: %v\n", err)
		return 1
	}
	fmt.Printf("config=%s\n", path)
	fmt.Printf("token=%s\n", token)
	fmt.Printf("listen=%s:%d\n", cfg.Bind, cfg.Port)
	fmt.Printf("capabilities=shell:%t,write:%t,delete:%t,desktop:%t,db:%t,dbWrite:%t\n",
		cfg.AllowShell, cfg.AllowWrite, cfg.AllowDelete, cfg.AllowDesktopControl, cfg.AllowDB, cfg.AllowDBWrite)
	fmt.Println("next=run `mode-agent serve --config <path>`; keep the token secret")
	return 0
}

func cmdToken(args []string) int {
	fs := flag.NewFlagSet("token", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	fmt.Println(cfg.Token)
	return 0
}

func cmdVersion(args []string) int {
	fmt.Printf("%s %s\n", agentName, version)
	fmt.Printf("go=%s os=%s arch=%s\n", goVersion, osName(), archName())
	return 0
}
