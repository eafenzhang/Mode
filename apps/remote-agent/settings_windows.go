//go:build windows
// +build windows

package main

import (
	"runtime"
	"strings"
	"sync"
	"syscall"
	"unsafe"
)

const idPairEdit = 1301

var settingsState struct {
	mu sync.Mutex
	ui *agentUIHandle
}

var pairState struct {
	mu       sync.Mutex
	code     string
	accepted bool
}

// showSettings 只负责本地 UI 开关；配对是独立入口。
func (ui *agentUIHandle) showSettings() {
	if ui == nil || ui.cfg == nil {
		return
	}
	settingsState.mu.Lock()
	if settingsState.ui != nil {
		settingsState.mu.Unlock()
		return
	}
	settingsState.ui = ui
	settingsState.mu.Unlock()

	ui.mu.Lock()
	owner := ui.hwnd
	ui.mu.Unlock()
	template := buildSettingsTemplate()
	callback := syscall.NewCallback(settingsDialogProc)
	instance, _, _ := procGetModuleHandleW.Call(0)
	procDialogBoxIndirectParamW.Call(
		instance,
		uintptr(unsafe.Pointer(&template[0])),
		owner,
		callback,
		0,
	)
	runtime.KeepAlive(template)
	runtime.KeepAlive(callback)

	settingsState.mu.Lock()
	settingsState.ui = nil
	settingsState.mu.Unlock()
}

func buildSettingsTemplate() []byte {
	var buffer []byte
	// DS_SETFONT | DS_MODALFRAME | DS_CENTER | WS_POPUP | WS_CAPTION | WS_SYSMENU
	putDWord(&buffer, 0x80C808C0)
	putDWord(&buffer, wsExTopmost)
	putWord(&buffer, 5)
	putWord(&buffer, 0)
	putWord(&buffer, 0)
	putWord(&buffer, 280)
	putWord(&buffer, 105)
	putWord(&buffer, 0) // no menu
	putWord(&buffer, 0) // default dialog class
	putUTF16(&buffer, "Mode Go 设置")
	putDialogFont(&buffer, 9, "Microsoft YaHei UI")
	addDialogItem(&buffer, 0x50000000, 12, 8, 256, 22, idInstallMessage, 0x0082,
		"开机自启与托盘设置。")
	addDialogItem(&buffer, 0x50010003, 14, 36, 252, 14, idInstallAutostart, 0x0080, "登录 Windows 时自动启动")
	addDialogItem(&buffer, 0x50010003, 14, 56, 252, 14, idInstallTray, 0x0080, "显示系统托盘图标")
	addDialogItem(&buffer, 0x50010001, 158, 80, 50, 16, 1, 0x0080, "保存")
	addDialogItem(&buffer, 0x50010000, 214, 80, 50, 16, 2, 0x0080, "取消")
	return buffer
}

func settingsDialogProc(hwnd, message, wParam, lParam uintptr) uintptr {
	if brush, handled := handleDialogColor(message, wParam, lParam); handled {
		return brush
	}
	switch message {
	case 0x0110: // WM_INITDIALOG
		setDialogIcon(hwnd)
		makeDialogTopmost(hwnd)
		setDialogHeaderFont(hwnd, idInstallMessage)
		settingsState.mu.Lock()
		ui := settingsState.ui
		settingsState.mu.Unlock()
		if ui != nil && ui.cfg != nil {
			setDialogCheck(hwnd, idInstallAutostart, ui.cfg.Panel.Autostart)
			setDialogCheck(hwnd, idInstallTray, ui.cfg.Panel.ShowTray)
		}
		return 1
	case 0x0111: // WM_COMMAND
		switch uint16(wParam & 0xFFFF) {
		case 1: // IDOK
			settingsState.mu.Lock()
			ui := settingsState.ui
			settingsState.mu.Unlock()
			if ui != nil {
				ui.applySettings(
					dialogChecked(hwnd, idInstallAutostart),
					dialogChecked(hwnd, idInstallTray),
				)
			}
			procEndDialog.Call(hwnd, 1)
			return 1
		case 2: // IDCANCEL
			procEndDialog.Call(hwnd, 0)
			return 1
		}
	case 0x0010: // WM_CLOSE
		procEndDialog.Call(hwnd, 0)
		return 1
	}
	return 0
}

// showPairing 是独立的配对入口：只输入 6 位配对码，桌面地址由 LAN/Tailscale 发现。
func (ui *agentUIHandle) showPairing() {
	if ui == nil || ui.cfg == nil {
		return
	}
	ui.mu.Lock()
	owner := ui.hwnd
	ui.mu.Unlock()
	code, ok := promptPairCode(owner)
	if !ok {
		return
	}
	if _, err := enrollWithCode(ui.cfg, code, ""); err != nil {
		showErrorDialog("Mode Go 配对失败", err.Error())
		return
	}
	showInfoDialog("Mode Go 配对成功", "已登记到 Mode 桌面端。\n请回到远程控制页点击「刷新列表」。")
}

func promptPairCode(owner uintptr) (string, bool) {
	template := buildPairTemplate()
	callback := syscall.NewCallback(pairDialogProc)
	instance, _, _ := procGetModuleHandleW.Call(0)
	result, _, _ := procDialogBoxIndirectParamW.Call(
		instance,
		uintptr(unsafe.Pointer(&template[0])),
		owner,
		callback,
		0,
	)
	runtime.KeepAlive(template)
	runtime.KeepAlive(callback)
	if int32(result) == -1 {
		return "", false
	}
	pairState.mu.Lock()
	defer pairState.mu.Unlock()
	return pairState.code, pairState.accepted
}

func buildPairTemplate() []byte {
	var buffer []byte
	// DS_SETFONT | DS_MODALFRAME | DS_CENTER | WS_POPUP | WS_CAPTION | WS_SYSMENU
	putDWord(&buffer, 0x80C808C0)
	putDWord(&buffer, wsExTopmost)
	putWord(&buffer, 5)
	putWord(&buffer, 0)
	putWord(&buffer, 0)
	putWord(&buffer, 280)
	putWord(&buffer, 118)
	putWord(&buffer, 0) // no menu
	putWord(&buffer, 0) // default dialog class
	putUTF16(&buffer, "Mode Go 配对")
	putDialogFont(&buffer, 9, "Microsoft YaHei UI")
	addDialogItem(&buffer, 0x50000000, 12, 8, 256, 20, idInstallMessage, 0x0082,
		"输入桌面端显示的 6 位配对码：")
	addDialogItem(&buffer, 0x50810080, 14, 32, 252, 16, idPairEdit, 0x0081, "")
	addDialogItem(&buffer, 0x50000000, 14, 54, 252, 14, idInstallHint, 0x0082,
		"配对码 5 分钟内有效，仅可使用一次。")
	addDialogItem(&buffer, 0x50010001, 158, 86, 50, 16, 1, 0x0080, "配对")
	addDialogItem(&buffer, 0x50010000, 214, 86, 50, 16, 2, 0x0080, "取消")
	return buffer
}

func pairDialogProc(hwnd, message, wParam, lParam uintptr) uintptr {
	if brush, handled := handleDialogColor(message, wParam, lParam); handled {
		return brush
	}
	switch message {
	case 0x0110: // WM_INITDIALOG
		setDialogIcon(hwnd)
		makeDialogTopmost(hwnd)
		setDialogHeaderFont(hwnd, idInstallMessage)
		pairState.mu.Lock()
		pairState.code = ""
		pairState.accepted = false
		pairState.mu.Unlock()
		if edit, _, _ := procGetDlgItem.Call(hwnd, idPairEdit); edit != 0 {
			procSetFocus.Call(edit)
		}
		return 1
	case 0x0111: // WM_COMMAND
		switch uint16(wParam & 0xFFFF) {
		case 1: // IDOK
			buffer := make([]uint16, 64)
			length, _, _ := procGetDlgItemTextW.Call(
				hwnd,
				idPairEdit,
				uintptr(unsafe.Pointer(&buffer[0])),
				uintptr(len(buffer)),
			)
			code := strings.TrimSpace(syscall.UTF16ToString(buffer[:int(length)]))
			if code == "" {
				showInfoDialog("Mode Go 配对", "请输入 6 位配对码。")
				return 1
			}
			pairState.mu.Lock()
			pairState.code = code
			pairState.accepted = true
			pairState.mu.Unlock()
			procEndDialog.Call(hwnd, 1)
			return 1
		case 2: // IDCANCEL
			pairState.mu.Lock()
			pairState.accepted = false
			pairState.mu.Unlock()
			procEndDialog.Call(hwnd, 0)
			return 1
		}
	case 0x0010: // WM_CLOSE
		pairState.mu.Lock()
		pairState.accepted = false
		pairState.mu.Unlock()
		procEndDialog.Call(hwnd, 0)
		return 1
	}
	return 0
}

func (ui *agentUIHandle) applySettings(autostart, tray bool) {
	if ui.cfg == nil {
		return
	}
	wasAutostart := ui.cfg.Panel.Autostart
	ui.cfg.Panel.Autostart = autostart
	ui.cfg.Panel.ShowTray = tray
	// 面板是固定能力，始终启用；设置页只读展示。
	ui.cfg.Panel.Enabled = true
	if ui.cfg.configPath != "" {
		if err := saveConfig(ui.cfg, ui.cfg.configPath); err != nil {
			showErrorDialog("Mode Go", "保存设置失败：\n"+err.Error())
		}
	}
	if err := syncAutostartForPanel(ui.cfg, wasAutostart && !autostart); err != nil {
		showErrorDialog("Mode Go", "更新开机自启失败：\n"+err.Error())
	}
	ui.setTrayEnabled(tray)
}
