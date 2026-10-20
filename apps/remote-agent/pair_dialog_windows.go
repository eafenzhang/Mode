// +build windows

package main

func showPairCodeDialog(cfg *Config, code *PairCodeResult) {
	messageBox("Mode Agent 配对", formatPairMessage(cfg, code), mbOK|mbIconInformation)
}
