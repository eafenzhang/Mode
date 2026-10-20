package main

import (
	"fmt"
	"io/ioutil"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const pidFileName = "agent.pid"

func writePIDFile(dir string, pid int) error {
	if dir == "" {
		return fmt.Errorf("data dir is empty")
	}
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	return ioutil.WriteFile(filepath.Join(dir, pidFileName), []byte(strconv.Itoa(pid)+"\n"), 0600)
}

func readPIDFile(dir string) (int, error) {
	data, err := ioutil.ReadFile(filepath.Join(dir, pidFileName))
	if err != nil {
		return 0, err
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil || pid <= 0 {
		return 0, fmt.Errorf("invalid pid file")
	}
	return pid, nil
}

func removePIDFile(dir string) {
	if dir != "" {
		os.Remove(filepath.Join(dir, pidFileName))
	}
}
