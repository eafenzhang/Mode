// +build !windows

package main

import "unicode/utf8"

func decodeConsoleBytes(data []byte, mode string) string {
	if mode == "utf8" || mode == "utf-8" || mode == "auto" {
		if utf8.Valid(data) {
			return string(data)
		}
	}
	return sanitizeUTF8(data)
}
