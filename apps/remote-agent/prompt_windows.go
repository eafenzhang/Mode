//go:build windows
// +build windows

package main

import (
	"runtime"
	"sync"
	"syscall"
	"unicode/utf16"
	"unsafe"
)

const (
	idPromptLabel = 1001
	idPromptEdit  = 1002

	idInstallMessage   = 1101
	idInstallAutostart = 1102
	idInstallTray      = 1103
	idInstallHint      = 1105

	bmGetCheck   = 0x00F0
	bmSetCheck   = 0x00F1
	bstChecked   = 1
	bstUnchecked = 0

	dsSetFont        = 0x0040
	wsExTopmost      = 0x00000008
	wmSetFont        = 0x0030
	wmCtlColorBtn    = 0x0135
	wmCtlColorEdit   = 0x0133
	wmCtlColorDlg    = 0x0136
	wmCtlColorStatic = 0x0138
)

var (
	procDialogBoxIndirectParamW = user32.NewProc("DialogBoxIndirectParamW")
	procEndDialog               = user32.NewProc("EndDialog")
	procGetDlgItem              = user32.NewProc("GetDlgItem")
	procSetFocus                = user32.NewProc("SetFocus")
	procGetDlgItemTextW         = user32.NewProc("GetDlgItemTextW")
	procSendDlgItemMessageW     = user32.NewProc("SendDlgItemMessageW")
	procSendMessageW            = user32.NewProc("SendMessageW")
	procGetDlgCtrlID            = user32.NewProc("GetDlgCtrlID")
	procSetWindowPos            = user32.NewProc("SetWindowPos")
	procGetModuleHandleW        = kernel32.NewProc("GetModuleHandleW")
)

var dialogTitleFontOnce sync.Once
var dialogTitleFont uintptr

const (
	wmSetIcon = 0x0080
	iconSmall = 0
	iconBig   = 1
)

var promptState struct {
	mu       sync.Mutex
	result   string
	accepted bool
}

var installOptionsState struct {
	mu        sync.Mutex
	autostart bool
	tray      bool
	accepted  bool
}

func promptForText(title, label, initial string) (string, bool) {
	template := buildPromptTemplate(title, label, initial)
	if len(template) == 0 {
		return "", false
	}
	promptState.mu.Lock()
	promptState.result = ""
	promptState.accepted = false
	promptState.mu.Unlock()
	callback := syscall.NewCallback(promptDialogProc)
	instance, _, _ := procGetModuleHandleW.Call(0)
	result, _, _ := procDialogBoxIndirectParamW.Call(
		instance,
		uintptr(unsafe.Pointer(&template[0])),
		0,
		callback,
		0,
	)
	runtime.KeepAlive(template)
	runtime.KeepAlive(callback)
	if int32(result) == -1 {
		return "", false
	}
	promptState.mu.Lock()
	defer promptState.mu.Unlock()
	return promptState.result, promptState.accepted
}

// promptInstallOptions 让安装器在落盘前选择自启、托盘与面板；返回的 ok=false 表示用户取消。
func promptInstallOptions(title, message, confirmLabel string, autostart, tray bool) (bool, bool, bool) {
	template := buildInstallOptionsTemplate(title, message, confirmLabel)
	if len(template) == 0 {
		return false, false, false
	}
	installOptionsState.mu.Lock()
	installOptionsState.autostart = autostart
	installOptionsState.tray = tray
	installOptionsState.accepted = false
	installOptionsState.mu.Unlock()
	callback := syscall.NewCallback(installOptionsDialogProc)
	instance, _, _ := procGetModuleHandleW.Call(0)
	result, _, _ := procDialogBoxIndirectParamW.Call(
		instance,
		uintptr(unsafe.Pointer(&template[0])),
		0,
		callback,
		0,
	)
	runtime.KeepAlive(template)
	runtime.KeepAlive(callback)
	if int32(result) == -1 {
		return false, false, false
	}
	installOptionsState.mu.Lock()
	defer installOptionsState.mu.Unlock()
	return installOptionsState.autostart, installOptionsState.tray, installOptionsState.accepted
}

func installOptionsDialogProc(hwnd, message, wParam, lParam uintptr) uintptr {
	if brush, handled := handleDialogColor(message, wParam, lParam); handled {
		return brush
	}
	switch message {
	case 0x0110: // WM_INITDIALOG
		setDialogIcon(hwnd)
		makeDialogTopmost(hwnd)
		setDialogHeaderFont(hwnd, idInstallMessage)
		installOptionsState.mu.Lock()
		autostart := installOptionsState.autostart
		tray := installOptionsState.tray
		installOptionsState.mu.Unlock()
		setDialogCheck(hwnd, idInstallAutostart, autostart)
		setDialogCheck(hwnd, idInstallTray, tray)
		return 1
	case 0x0111: // WM_COMMAND
		switch uint16(wParam & 0xFFFF) {
		case 1: // IDOK
			installOptionsState.mu.Lock()
			installOptionsState.autostart = dialogChecked(hwnd, idInstallAutostart)
			installOptionsState.tray = dialogChecked(hwnd, idInstallTray)
			installOptionsState.accepted = true
			installOptionsState.mu.Unlock()
			procEndDialog.Call(hwnd, 1)
			return 1
		case 2: // IDCANCEL
			installOptionsState.mu.Lock()
			installOptionsState.accepted = false
			installOptionsState.mu.Unlock()
			procEndDialog.Call(hwnd, 0)
			return 1
		}
	case 0x0010: // WM_CLOSE
		installOptionsState.mu.Lock()
		installOptionsState.accepted = false
		installOptionsState.mu.Unlock()
		procEndDialog.Call(hwnd, 0)
		return 1
	}
	return 0
}

func setDialogIcon(hwnd uintptr) {
	icon := loadAgentIcon()
	if icon == 0 {
		return
	}
	procSendMessageW.Call(hwnd, wmSetIcon, iconSmall, icon)
	procSendMessageW.Call(hwnd, wmSetIcon, iconBig, icon)
}

func setDialogCheck(hwnd uintptr, id uint16, checked bool) {
	value := uintptr(bstUnchecked)
	if checked {
		value = bstChecked
	}
	procSendDlgItemMessageW.Call(hwnd, uintptr(id), bmSetCheck, value, 0)
}

func dialogChecked(hwnd uintptr, id uint16) bool {
	value, _, _ := procSendDlgItemMessageW.Call(hwnd, uintptr(id), bmGetCheck, 0, 0)
	return value == bstChecked
}

func putDialogFont(buffer *[]byte, points uint16, face string) {
	putWord(buffer, points)
	putUTF16(buffer, face)
}

// makeDialogTopmost 让对话框始终浮在面板/其它应用之上。
func makeDialogTopmost(hwnd uintptr) {
	procSetWindowPos.Call(
		hwnd,
		^uintptr(0), // HWND_TOPMOST
		0,
		0,
		0,
		0,
		0x0001|0x0002|0x0040, // SWP_NOSIZE | SWP_NOMOVE | SWP_SHOWWINDOW
	)
}

func ensureDialogTitleFont() uintptr {
	dialogTitleFontOnce.Do(func() {
		dpi := systemDPI()
		dialogTitleFont = createFontWithFace("Microsoft YaHei UI", scaleDIP(16, dpi), fwBold)
	})
	return dialogTitleFont
}

func setDialogHeaderFont(hwnd uintptr, id uint16) {
	control, _, _ := procGetDlgItem.Call(hwnd, uintptr(id))
	if control == 0 {
		return
	}
	font := ensureDialogTitleFont()
	if font != 0 {
		procSendMessageW.Call(control, wmSetFont, font, 1)
	}
}

// handleDialogColor 统一对话框的白底、深色文字与浅灰提示。
func handleDialogColor(message, wParam, lParam uintptr) (uintptr, bool) {
	brush, _, _ := procGetStockObject.Call(whiteBkBrush)
	switch message {
	case wmCtlColorDlg, wmCtlColorBtn:
		return brush, true
	case wmCtlColorEdit:
		procSetTextColor.Call(wParam, uintptr(rgb(30, 30, 30)))
		return brush, true
	case wmCtlColorStatic:
		procSetBkMode.Call(wParam, transparentBkMode)
		id, _, _ := procGetDlgCtrlID.Call(lParam)
		if id == uintptr(idInstallHint) {
			procSetTextColor.Call(wParam, uintptr(rgb(110, 110, 110)))
		} else {
			procSetTextColor.Call(wParam, uintptr(rgb(35, 35, 35)))
		}
		return brush, true
	}
	return 0, false
}

func promptDialogProc(hwnd, message, wParam, lParam uintptr) uintptr {
	if brush, handled := handleDialogColor(message, wParam, lParam); handled {
		return brush
	}
	switch message {
	case 0x0110: // WM_INITDIALOG
		setDialogIcon(hwnd)
		makeDialogTopmost(hwnd)
		setDialogHeaderFont(hwnd, idPromptLabel)
		edit, _, _ := procGetDlgItem.Call(hwnd, idPromptEdit)
		if edit != 0 {
			procSetFocus.Call(edit)
			procSendDlgItemMessageW.Call(hwnd, idPromptEdit, 0x00B1, 0, ^uintptr(0)) // EM_SETSEL
		}
		return 1
	case 0x0111: // WM_COMMAND
		switch uint16(wParam & 0xFFFF) {
		case 1: // IDOK
			buffer := make([]uint16, 4096)
			length, _, _ := procGetDlgItemTextW.Call(
				hwnd,
				idPromptEdit,
				uintptr(unsafe.Pointer(&buffer[0])),
				uintptr(len(buffer)),
			)
			promptState.mu.Lock()
			promptState.result = syscall.UTF16ToString(buffer[:int(length)])
			promptState.accepted = true
			promptState.mu.Unlock()
			procEndDialog.Call(hwnd, 1)
			return 1
		case 2: // IDCANCEL
			promptState.mu.Lock()
			promptState.accepted = false
			promptState.mu.Unlock()
			procEndDialog.Call(hwnd, 0)
			return 1
		}
	case 0x0010: // WM_CLOSE
		procEndDialog.Call(hwnd, 0)
		return 1
	}
	return 0
}

func align4(buffer *[]byte) {
	for len(*buffer)%4 != 0 {
		*buffer = append(*buffer, 0)
	}
}

func putWord(buffer *[]byte, value uint16) {
	*buffer = append(*buffer, byte(value), byte(value>>8))
}

func putDWord(buffer *[]byte, value uint32) {
	*buffer = append(*buffer, byte(value), byte(value>>8), byte(value>>16), byte(value>>24))
}

func putUTF16(buffer *[]byte, value string) {
	for _, unit := range utf16.Encode([]rune(value)) {
		putWord(buffer, unit)
	}
	putWord(buffer, 0)
}

func addDialogItem(buffer *[]byte, style uint32, x, y, cx, cy, id, classAtom uint16, text string) {
	align4(buffer)
	putDWord(buffer, style)
	putDWord(buffer, 0)
	putWord(buffer, x)
	putWord(buffer, y)
	putWord(buffer, cx)
	putWord(buffer, cy)
	putWord(buffer, id)
	putWord(buffer, 0xFFFF)
	putWord(buffer, classAtom)
	putUTF16(buffer, text)
	putWord(buffer, 0) // no creation data
}

func buildPromptTemplate(title, label, initial string) []byte {
	var buffer []byte
	// DS_SETFONT | DS_MODALFRAME | DS_CENTER | WS_POPUP | WS_CAPTION | WS_SYSMENU
	putDWord(&buffer, 0x80C808C0)
	putDWord(&buffer, wsExTopmost)
	putWord(&buffer, 4)
	putWord(&buffer, 0)
	putWord(&buffer, 0)
	putWord(&buffer, 340)
	putWord(&buffer, 140)
	putWord(&buffer, 0) // no menu
	putWord(&buffer, 0) // default dialog class
	putUTF16(&buffer, title)
	putDialogFont(&buffer, 9, "Microsoft YaHei UI")
	addDialogItem(&buffer, 0x50000000, 10, 10, 320, 32, idPromptLabel, 0x0082, label)
	addDialogItem(&buffer, 0x50810080, 10, 50, 320, 14, idPromptEdit, 0x0081, initial)
	addDialogItem(&buffer, 0x50010001, 210, 92, 55, 16, 1, 0x0080, "确定")
	addDialogItem(&buffer, 0x50010000, 270, 92, 55, 16, 2, 0x0080, "取消")
	return buffer
}

func buildInstallOptionsTemplate(title, message, confirmLabel string) []byte {
	var buffer []byte
	// DS_SETFONT | DS_MODALFRAME | DS_CENTER | WS_POPUP | WS_CAPTION | WS_SYSMENU
	putDWord(&buffer, 0x80C808C0)
	putDWord(&buffer, wsExTopmost)
	putWord(&buffer, 5)
	putWord(&buffer, 0)
	putWord(&buffer, 0)
	putWord(&buffer, 360)
	putWord(&buffer, 175)
	putWord(&buffer, 0) // no menu
	putWord(&buffer, 0) // default dialog class
	putUTF16(&buffer, title)
	putDialogFont(&buffer, 9, "Microsoft YaHei UI")
	addDialogItem(&buffer, 0x50000000, 12, 12, 336, 60, idInstallMessage, 0x0082, message)
	addDialogItem(&buffer, 0x50010003, 16, 80, 328, 16, idInstallAutostart, 0x0080, "登录 Windows 时自动启动")
	addDialogItem(&buffer, 0x50010003, 16, 102, 328, 16, idInstallTray, 0x0080, "显示系统托盘图标")
	addDialogItem(&buffer, 0x50010001, 225, 145, 55, 16, 1, 0x0080, confirmLabel)
	addDialogItem(&buffer, 0x50010000, 290, 145, 55, 16, 2, 0x0080, "取消")
	return buffer
}

func showInfoDialog(title, text string) {
	messageBox(title, text, mbOK|mbIconInformation)
}

func showErrorDialog(title, text string) {
	messageBox(title, text, mbOK|0x00000010) // MB_ICONERROR
}
