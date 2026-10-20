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
)

var (
	procDialogBoxIndirectParamW = user32.NewProc("DialogBoxIndirectParamW")
	procEndDialog               = user32.NewProc("EndDialog")
	procGetDlgItem              = user32.NewProc("GetDlgItem")
	procSetFocus                = user32.NewProc("SetFocus")
	procGetDlgItemTextW         = user32.NewProc("GetDlgItemTextW")
	procSendDlgItemMessageW     = user32.NewProc("SendDlgItemMessageW")
	procGetModuleHandleW        = kernel32.NewProc("GetModuleHandleW")
)

var promptState struct {
	mu       sync.Mutex
	result   string
	accepted bool
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

func promptDialogProc(hwnd, message, wParam, lParam uintptr) uintptr {
	switch message {
	case 0x0110: // WM_INITDIALOG
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
	// DS_MODALFRAME | DS_CENTER | WS_POPUP | WS_CAPTION | WS_SYSMENU
	putDWord(&buffer, 0x80C80880)
	putDWord(&buffer, 0)
	putWord(&buffer, 4)
	putWord(&buffer, 0)
	putWord(&buffer, 0)
	putWord(&buffer, 340)
	putWord(&buffer, 140)
	putWord(&buffer, 0) // no menu
	putWord(&buffer, 0) // default dialog class
	putUTF16(&buffer, title)
	addDialogItem(&buffer, 0x50000000, 10, 10, 320, 32, idPromptLabel, 0x0082, label)
	addDialogItem(&buffer, 0x50810080, 10, 50, 320, 14, idPromptEdit, 0x0081, initial)
	addDialogItem(&buffer, 0x50010001, 210, 92, 55, 16, 1, 0x0080, "确定")
	addDialogItem(&buffer, 0x50010000, 270, 92, 55, 16, 2, 0x0080, "取消")
	return buffer
}

func showInfoDialog(title, text string) {
	messageBox(title, text, mbOK|mbIconInformation)
}

func showErrorDialog(title, text string) {
	messageBox(title, text, mbOK|0x00000010) // MB_ICONERROR
}
