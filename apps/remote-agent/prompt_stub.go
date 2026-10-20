// +build !windows

package main

import (
	"fmt"
	"os"
)

func promptForText(title, label, initial string) (string, bool) {
	return "", false
}

func showInfoDialog(title, text string) {
	fmt.Printf("%s: %s\n", title, text)
}

func showErrorDialog(title, text string) {
	fmt.Fprintf(os.Stderr, "%s: %s\n", title, text)
}
