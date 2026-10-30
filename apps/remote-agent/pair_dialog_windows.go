// +build windows

package main

func showPairCodeDialog(cfg *Config, code *PairCodeResult) {
	messageBox("Mode Go 配对", formatPairMessage(cfg, code), mbOK|mbIconInformation)
}
