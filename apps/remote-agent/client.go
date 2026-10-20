package main

import (
	"bytes"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"io/ioutil"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

type remoteFlags struct {
	host        string
	token       string
	tokenFile   string
	tlsEnabled  bool
	fingerprint string
	insecure    bool
	timeoutSec  int
}

type remoteClient struct {
	baseURL string
	token   string
	client  *http.Client
}

type clientEnvelope struct {
	OK     bool            `json:"ok"`
	Result json.RawMessage `json:"result"`
	Error  *APIError       `json:"error"`
}

func addRemoteFlags(fs *flag.FlagSet, flags *remoteFlags) {
	fs.StringVar(&flags.host, "host", "127.0.0.1:47821", "agent host:port or URL")
	fs.StringVar(&flags.token, "token", "", "bearer token (prefer --token-file or MODE_AGENT_TOKEN)")
	fs.StringVar(&flags.tokenFile, "token-file", "", "file containing the bearer token")
	fs.BoolVar(&flags.tlsEnabled, "tls", false, "use HTTPS")
	fs.StringVar(&flags.fingerprint, "tls-fingerprint", "", "expected SHA-256 certificate fingerprint")
	fs.BoolVar(&flags.insecure, "insecure", false, "skip TLS certificate verification")
	fs.IntVar(&flags.timeoutSec, "timeout", 60, "request timeout in seconds")
}

func (f remoteFlags) resolveToken() (string, error) {
	if strings.TrimSpace(f.token) != "" {
		return strings.TrimSpace(f.token), nil
	}
	if strings.TrimSpace(f.tokenFile) != "" {
		data, err := ioutil.ReadFile(f.tokenFile)
		if err != nil {
			return "", err
		}
		lines := strings.Split(string(data), "\n")
		if len(lines) == 0 {
			return "", fmt.Errorf("token file is empty")
		}
		return strings.TrimSpace(lines[0]), nil
	}
	if value := strings.TrimSpace(os.Getenv("MODE_AGENT_TOKEN")); value != "" {
		return value, nil
	}
	return "", fmt.Errorf("token is required: use --token-file, --token, or MODE_AGENT_TOKEN")
}

func newRemoteClient(flags remoteFlags) (*remoteClient, error) {
	token, err := flags.resolveToken()
	if err != nil {
		return nil, err
	}
	host := strings.TrimSpace(flags.host)
	if host == "" {
		return nil, fmt.Errorf("host is required")
	}
	scheme := "http"
	if flags.tlsEnabled {
		scheme = "https"
	}
	if strings.Contains(host, "://") {
		parsed, parseErr := url.Parse(host)
		if parseErr != nil {
			return nil, parseErr
		}
		scheme = parsed.Scheme
		host = parsed.Host
	}
	if scheme != "http" && scheme != "https" {
		return nil, fmt.Errorf("unsupported scheme: %s", scheme)
	}
	tlsConfig := &tls.Config{}
	if scheme == "https" {
		if strings.TrimSpace(flags.fingerprint) != "" {
			expected, parseErr := parseFingerprint(flags.fingerprint)
			if parseErr != nil {
				return nil, parseErr
			}
			tlsConfig.InsecureSkipVerify = true
			tlsConfig.VerifyPeerCertificate = func(rawCerts [][]byte, _ [][]*x509.Certificate) error {
				if len(rawCerts) == 0 {
					return fmt.Errorf("no peer certificate")
				}
				sum := sha256.Sum256(rawCerts[0])
				if !bytes.Equal(sum[:], expected) {
					return fmt.Errorf("TLS certificate fingerprint mismatch")
				}
				return nil
			}
		} else if flags.insecure {
			tlsConfig.InsecureSkipVerify = true
		}
	} else if flags.fingerprint != "" || flags.insecure {
		return nil, fmt.Errorf("TLS flags require --tls or an https:// host")
	}
	timeout := time.Duration(flags.timeoutSec) * time.Second
	if timeout <= 0 {
		timeout = 60 * time.Second
	}
	transport := &http.Transport{TLSClientConfig: tlsConfig}
	return &remoteClient{
		baseURL: scheme + "://" + host,
		token:   token,
		client:  &http.Client{Timeout: timeout, Transport: transport},
	}, nil
}

func parseFingerprint(value string) ([]byte, error) {
	value = strings.TrimSpace(value)
	value = strings.TrimPrefix(value, "SHA256:")
	value = strings.TrimPrefix(value, "sha256:")
	value = strings.Replace(value, ":", "", -1)
	decoded, err := hex.DecodeString(value)
	if err != nil || len(decoded) != 32 {
		return nil, fmt.Errorf("invalid SHA-256 fingerprint")
	}
	return decoded, nil
}

func (c *remoteClient) doRaw(method, path string, body interface{}) ([]byte, *APIError) {
	var reader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return nil, apiError("client_error", err.Error(), 0)
		}
		reader = bytes.NewReader(data)
	}
	request, err := http.NewRequest(method, c.baseURL+path, reader)
	if err != nil {
		return nil, apiError("client_error", err.Error(), 0)
	}
	request.Header.Set("Authorization", "Bearer "+c.token)
	request.Header.Set("Accept", "application/json")
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	response, err := c.client.Do(request)
	if err != nil {
		return nil, apiError("network_error", err.Error(), 0)
	}
	defer response.Body.Close()
	data, err := ioutil.ReadAll(io.LimitReader(response.Body, 32<<20))
	if err != nil {
		return nil, apiError("network_error", err.Error(), 0)
	}
	if response.StatusCode >= 400 {
		var envelope clientEnvelope
		if json.Unmarshal(data, &envelope) == nil && envelope.Error != nil {
			return nil, envelope.Error
		}
		return nil, apiError("http_error", fmt.Sprintf("HTTP %d: %s", response.StatusCode, strings.TrimSpace(string(data))), response.StatusCode)
	}
	return data, nil
}

func (c *remoteClient) result(method, path string, body interface{}, output interface{}) *APIError {
	data, apiErr := c.doRaw(method, path, body)
	if apiErr != nil {
		return apiErr
	}
	var envelope clientEnvelope
	if err := json.Unmarshal(data, &envelope); err != nil {
		return apiError("client_error", "invalid response: "+err.Error(), 0)
	}
	if !envelope.OK {
		if envelope.Error != nil {
			return envelope.Error
		}
		return apiError("client_error", "request failed", 0)
	}
	if output != nil && len(envelope.Result) > 0 {
		if err := json.Unmarshal(envelope.Result, output); err != nil {
			return apiError("client_error", "invalid result: "+err.Error(), 0)
		}
	}
	return nil
}

func printJSON(value interface{}) {
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return
	}
	fmt.Println(string(data))
}

func fail(apiErr *APIError) int {
	if apiErr == nil {
		return 1
	}
	fmt.Fprintf(os.Stderr, "%s: %s\n", apiErr.Code, apiErr.Message)
	return 1
}

func cmdRemote(args []string) int {
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "remote subcommand is required (info, exec, fs-list, fs-read, fs-write, fs-stat, fs-remove, ps, kill, screen, mouse, key, windows, focus, db-drivers, db-query, mcp-tools, raw)")
		return 2
	}
	command := args[0]
	rest := args[1:]
	switch command {
	case "info":
		return remoteInfo(rest)
	case "exec":
		return remoteExec(rest)
	case "job-start":
		return remoteJobStart(rest)
	case "job-status":
		return remoteJobStatus(rest)
	case "job-write":
		return remoteJobWrite(rest)
	case "job-cancel":
		return remoteJobCancel(rest)
	case "job-list":
		return remoteJobList(rest)
	case "fs-list":
		return remoteFSList(rest)
	case "fs-read":
		return remoteFSRead(rest)
	case "fs-write":
		return remoteFSWrite(rest)
	case "fs-stat":
		return remoteFSStat(rest)
	case "fs-remove":
		return remoteFSRemove(rest)
	case "ps":
		return remoteProcesses(rest)
	case "kill":
		return remoteKill(rest)
	case "screen":
		return remoteScreen(rest)
	case "mouse":
		return remoteMouse(rest)
	case "key":
		return remoteKeyboard(rest)
	case "windows":
		return remoteWindows(rest)
	case "focus":
		return remoteFocus(rest)
	case "db-drivers":
		return remoteDBDrivers(rest)
	case "db-query":
		return remoteDBQuery(rest)
	case "mcp-tools":
		return remoteMCPTools(rest)
	case "raw":
		return remoteRaw(rest)
	default:
		fmt.Fprintf(os.Stderr, "unknown remote subcommand: %s\n", command)
		return 2
	}
}

func remoteInfo(args []string) int {
	fs := flag.NewFlagSet("remote info", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result InfoResult
	if apiErr := client.result("GET", "/v1/info", nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteExec(args []string) int {
	fs := flag.NewFlagSet("remote exec", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	command := fs.String("command", "", "command to run")
	shell := fs.String("shell", "cmd", "cmd, powershell, or none")
	cwd := fs.String("cwd", "", "working directory")
	stdin := fs.String("stdin", "", "stdin text")
	encoding := fs.String("encoding", "auto", "auto, utf8, oem, or ansi")
	timeoutMS := fs.Int("command-timeout", 60000, "remote command timeout in milliseconds")
	maxOutput := fs.Int("max-output", 0, "max output bytes")
	exitCode := fs.Bool("exit-code", false, "exit with the remote command's exit code")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if needed := *timeoutMS/1000 + 15; needed > flags.timeoutSec {
		flags.timeoutSec = needed
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	req := ExecRequest{
		Command:        *command,
		Shell:          *shell,
		Cwd:            *cwd,
		Stdin:          *stdin,
		Encoding:       *encoding,
		TimeoutMS:      *timeoutMS,
		MaxOutputBytes: *maxOutput,
	}
	var result ExecResult
	if apiErr := client.result("POST", "/v1/exec", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	if *exitCode {
		return result.ExitCode
	}
	return 0
}

func remoteJobStart(args []string) int {
	fs := flag.NewFlagSet("remote job-start", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	command := fs.String("command", "", "command to run")
	shell := fs.String("shell", "cmd", "cmd, powershell, or none")
	cwd := fs.String("cwd", "", "working directory")
	stdin := fs.String("stdin", "", "stdin text")
	encoding := fs.String("encoding", "auto", "auto, utf8, oem, or ansi")
	timeoutMS := fs.Int("command-timeout", 0, "remote command timeout in milliseconds (0 = no timeout)")
	maxOutput := fs.Int("max-output", 0, "max output bytes")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	req := ExecRequest{
		Command:        *command,
		Shell:          *shell,
		Cwd:            *cwd,
		Stdin:          *stdin,
		Encoding:       *encoding,
		TimeoutMS:      *timeoutMS,
		MaxOutputBytes: *maxOutput,
	}
	var result JobStatusResult
	if apiErr := client.result("POST", "/v1/jobs", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteJobStatus(args []string) int {
	fs := flag.NewFlagSet("remote job-status", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	jobID := fs.String("job", "", "job id")
	stdoutOffset := fs.Int64("stdout-offset", 0, "stdout offset")
	stderrOffset := fs.Int64("stderr-offset", 0, "stderr offset")
	encoding := fs.String("encoding", "", "override output encoding")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	path := "/v1/jobs/" + url.PathEscape(*jobID) + "?stdoutOffset=" + strconv.FormatInt(*stdoutOffset, 10) + "&stderrOffset=" + strconv.FormatInt(*stderrOffset, 10)
	if *encoding != "" {
		path += "&encoding=" + url.QueryEscape(*encoding)
	}
	var result JobStatusResult
	if apiErr := client.result("GET", path, nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteJobWrite(args []string) int {
	fs := flag.NewFlagSet("remote job-write", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	jobID := fs.String("job", "", "job id")
	data := fs.String("data", "", "stdin data")
	base64Data := fs.Bool("base64", false, "decode data as base64")
	closeAfter := fs.Bool("close", false, "close stdin after writing")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	encoding := "text"
	if *base64Data {
		encoding = "base64"
	}
	req := JobStdinRequest{Data: *data, Encoding: encoding, Close: *closeAfter}
	var result JobStdinResult
	if apiErr := client.result("POST", "/v1/jobs/"+url.PathEscape(*jobID)+"/stdin", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteJobCancel(args []string) int {
	fs := flag.NewFlagSet("remote job-cancel", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	jobID := fs.String("job", "", "job id")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result JobStatusResult
	if apiErr := client.result("POST", "/v1/jobs/"+url.PathEscape(*jobID)+"/cancel", map[string]interface{}{}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteJobList(args []string) int {
	fs := flag.NewFlagSet("remote job-list", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result JobListResult
	if apiErr := client.result("GET", "/v1/jobs", nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteFSList(args []string) int {
	fs := flag.NewFlagSet("remote fs-list", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	path := fs.String("path", "", "directory path")
	maxEntries := fs.Int("max-entries", 0, "maximum entries")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result FSListResult
	if apiErr := client.result("POST", "/v1/fs/list", FSListRequest{Path: *path, MaxEntries: *maxEntries}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteFSRead(args []string) int {
	fs := flag.NewFlagSet("remote fs-read", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	path := fs.String("path", "", "file path")
	offset := fs.Int64("offset", 0, "byte offset")
	maxBytes := fs.Int("max-bytes", 0, "maximum bytes")
	encoding := fs.String("encoding", "auto", "auto, utf8, oem, ansi, or binary")
	out := fs.String("out", "", "write file content to this local path")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result FSReadResult
	if apiErr := client.result("POST", "/v1/fs/read", FSReadRequest{Path: *path, Offset: *offset, MaxBytes: *maxBytes, Encoding: *encoding}, &result); apiErr != nil {
		return fail(apiErr)
	}
	if *out != "" {
		var data []byte
		if result.Base64 != "" {
			decoded, decodeErr := base64.StdEncoding.DecodeString(result.Base64)
			if decodeErr != nil {
				return fail(apiError("client_error", decodeErr.Error(), 0))
			}
			data = decoded
		} else {
			data = []byte(result.Text)
		}
		if writeErr := ioutil.WriteFile(*out, data, 0600); writeErr != nil {
			return fail(apiError("client_error", writeErr.Error(), 0))
		}
		printJSON(map[string]interface{}{
			"path":       result.Path,
			"size":       result.Size,
			"offset":     result.Offset,
			"nextOffset": result.NextOffset,
			"encoding":   result.Encoding,
			"truncated":  result.Truncated,
			"out":        *out,
		})
		return 0
	}
	printJSON(result)
	return 0
}

func remoteFSWrite(args []string) int {
	fs := flag.NewFlagSet("remote fs-write", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	path := fs.String("path", "", "remote file path")
	file := fs.String("file", "", "local file to upload")
	text := fs.String("text", "", "text payload")
	appendData := fs.Bool("append", false, "append instead of replace")
	createDirs := fs.Bool("create-dirs", false, "create parent directories")
	expected := fs.String("sha256", "", "expected SHA-256 of the payload")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	req := FSWriteRequest{Path: *path, Text: *text, Append: *appendData, CreateDirs: *createDirs, ExpectedSHA256: *expected}
	if *file != "" {
		data, readErr := ioutil.ReadFile(*file)
		if readErr != nil {
			return fail(apiError("client_error", readErr.Error(), 0))
		}
		req.Text = ""
		req.Base64 = base64.StdEncoding.EncodeToString(data)
	}
	var result FSWriteResult
	if apiErr := client.result("POST", "/v1/fs/write", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteFSStat(args []string) int {
	fs := flag.NewFlagSet("remote fs-stat", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	path := fs.String("path", "", "file or directory path")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result FSStatResult
	if apiErr := client.result("POST", "/v1/fs/stat", FSListRequest{Path: *path}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteFSRemove(args []string) int {
	fs := flag.NewFlagSet("remote fs-remove", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	path := fs.String("path", "", "file or directory path")
	recursive := fs.Bool("recursive", false, "delete directory recursively")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result FSRemoveResult
	if apiErr := client.result("POST", "/v1/fs/remove", FSRemoveRequest{Path: *path, Recursive: *recursive}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteProcesses(args []string) int {
	fs := flag.NewFlagSet("remote ps", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result ProcessListResult
	if apiErr := client.result("GET", "/v1/processes", nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteKill(args []string) int {
	fs := flag.NewFlagSet("remote kill", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	pid := fs.Uint("pid", 0, "process id")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result ProcessKillResult
	if apiErr := client.result("POST", "/v1/process/kill", ProcessKillRequest{PID: uint32(*pid)}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteScreen(args []string) int {
	fs := flag.NewFlagSet("remote screen", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	maxWidth := fs.Int("max-width", 0, "maximum width")
	maxHeight := fs.Int("max-height", 0, "maximum height")
	out := fs.String("out", "screen.png", "local PNG path")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result ScreenResult
	if apiErr := client.result("POST", "/v1/screen/capture", ScreenRequest{MaxWidth: *maxWidth, MaxHeight: *maxHeight}, &result); apiErr != nil {
		return fail(apiErr)
	}
	data, decodeErr := base64.StdEncoding.DecodeString(result.PNGBase64)
	if decodeErr != nil {
		return fail(apiError("client_error", decodeErr.Error(), 0))
	}
	if writeErr := ioutil.WriteFile(*out, data, 0600); writeErr != nil {
		return fail(apiError("client_error", writeErr.Error(), 0))
	}
	printJSON(map[string]interface{}{
		"width":  result.Width,
		"height": result.Height,
		"bytes":  result.Bytes,
		"out":    *out,
	})
	return 0
}

func remoteMouse(args []string) int {
	fs := flag.NewFlagSet("remote mouse", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	action := fs.String("action", "click", "move, click, down, up, scroll, or drag")
	x := fs.Int("x", 0, "x coordinate")
	y := fs.Int("y", 0, "y coordinate")
	x1 := fs.Int("x1", 0, "drag start x")
	y1 := fs.Int("y1", 0, "drag start y")
	x2 := fs.Int("x2", 0, "drag end x")
	y2 := fs.Int("y2", 0, "drag end y")
	button := fs.String("button", "left", "left, right, or middle")
	delta := fs.Int("delta", 0, "scroll notches")
	duration := fs.Int("duration", 300, "drag duration in milliseconds")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	req := MouseRequest{
		Action: *action, X: *x, Y: *y, X1: *x1, Y1: *y1, X2: *x2, Y2: *y2,
		Button: *button, Delta: *delta, DurationMS: *duration,
	}
	var result MouseResult
	if apiErr := client.result("POST", "/v1/input/mouse", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteKeyboard(args []string) int {
	fs := flag.NewFlagSet("remote key", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	action := fs.String("action", "", "type or key")
	text := fs.String("text", "", "text to type")
	key := fs.String("key", "", "key combination, e.g. CTRL+S")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result KeyboardResult
	if apiErr := client.result("POST", "/v1/input/key", KeyboardRequest{Action: *action, Text: *text, Key: *key}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteWindows(args []string) int {
	fs := flag.NewFlagSet("remote windows", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result WindowListResult
	if apiErr := client.result("GET", "/v1/windows", nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteFocus(args []string) int {
	fs := flag.NewFlagSet("remote focus", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	hwnd := fs.String("hwnd", "", "window handle from remote windows")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result WindowFocusResult
	if apiErr := client.result("POST", "/v1/window/focus", WindowFocusRequest{HWND: *hwnd}, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteDBDrivers(args []string) int {
	fs := flag.NewFlagSet("remote db-drivers", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var result DBDriversResult
	if apiErr := client.result("GET", "/v1/db/drivers", nil, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteDBQuery(args []string) int {
	fs := flag.NewFlagSet("remote db-query", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	dsn := fs.String("dsn", "", "named DSN configured on the agent")
	connection := fs.String("connection", "", "ODBC connection string (avoid sending credentials)")
	sqlText := fs.String("sql", "", "SQL statement")
	maxRows := fs.Int("max-rows", 200, "maximum rows")
	timeoutMS := fs.Int("query-timeout", 30000, "query timeout in milliseconds")
	write := fs.Bool("write", false, "allow a write statement (requires allowDbWrite)")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if needed := *timeoutMS/1000 + 15; needed > flags.timeoutSec {
		flags.timeoutSec = needed
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	readOnly := !*write
	req := DBQueryRequest{
		DSN: *dsn, Connection: *connection, SQL: *sqlText,
		MaxRows: *maxRows, TimeoutMS: *timeoutMS, ReadOnly: &readOnly,
	}
	var result DBQueryResult
	if apiErr := client.result("POST", "/v1/db/query", req, &result); apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func remoteMCPTools(args []string) int {
	fs := flag.NewFlagSet("remote mcp-tools", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	initialize := map[string]interface{}{
		"protocolVersion": mcpLatestVersion,
		"capabilities":    map[string]interface{}{},
		"clientInfo":      map[string]string{"name": "mode-agent-cli", "version": version},
	}
	if _, apiErr := client.mcpCall("initialize", initialize); apiErr != nil {
		return fail(apiErr)
	}
	result, apiErr := client.mcpCall("tools/list", map[string]interface{}{})
	if apiErr != nil {
		return fail(apiErr)
	}
	printJSON(result)
	return 0
}

func (c *remoteClient) mcpCall(method string, params interface{}) (json.RawMessage, *APIError) {
	request := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      1,
		"method":  method,
		"params":  params,
	}
	data, apiErr := c.doRaw("POST", "/mcp", request)
	if apiErr != nil {
		return nil, apiErr
	}
	var response struct {
		JSONRPC string          `json:"jsonrpc"`
		ID      json.RawMessage `json:"id"`
		Result  json.RawMessage `json:"result"`
		Error   *mcpError       `json:"error"`
	}
	if err := json.Unmarshal(data, &response); err != nil {
		return nil, apiError("client_error", "invalid MCP response: "+err.Error(), 0)
	}
	if response.Error != nil {
		return nil, apiError("mcp_error", response.Error.Message, 0)
	}
	return response.Result, nil
}

func remoteRaw(args []string) int {
	fs := flag.NewFlagSet("remote raw", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	flags := remoteFlags{}
	addRemoteFlags(fs, &flags)
	method := fs.String("method", "POST", "HTTP method")
	path := fs.String("path", "/v1/info", "request path")
	body := fs.String("body", "", "raw JSON body")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	client, err := newRemoteClient(flags)
	if err != nil {
		return fail(apiError("client_error", err.Error(), 0))
	}
	var payload interface{}
	if strings.TrimSpace(*body) != "" {
		if err := json.Unmarshal([]byte(*body), &payload); err != nil {
			return fail(apiError("client_error", "invalid JSON body: "+err.Error(), 0))
		}
	}
	data, apiErr := client.doRaw(strings.ToUpper(*method), *path, payload)
	if apiErr != nil {
		return fail(apiErr)
	}
	fmt.Println(string(data))
	return 0
}
