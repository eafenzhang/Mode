// +build windows

package main

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"image"
	"image/png"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode/utf8"
	"unsafe"
)

const (
	mouseMove        = 0x0001
	mouseLeftDown    = 0x0002
	mouseLeftUp      = 0x0004
	mouseRightDown   = 0x0008
	mouseRightUp     = 0x0010
	mouseMiddleDown  = 0x0020
	mouseMiddleUp    = 0x0040
	mouseWheel       = 0x0800
	mouseAbsolute    = 0x8000
	mouseVirtualDesk = 0x4000
)

func inputRecordSize() int {
	if unsafe.Sizeof(uintptr(0)) == 8 {
		return 40
	}
	return 28
}

func inputUnionOffset() int {
	if unsafe.Sizeof(uintptr(0)) == 8 {
		return 8
	}
	return 4
}

func mouseExtraOffset() int {
	if unsafe.Sizeof(uintptr(0)) == 8 {
		return inputUnionOffset() + 24
	}
	return inputUnionOffset() + 20
}

func keyExtraOffset() int {
	if unsafe.Sizeof(uintptr(0)) == 8 {
		return inputUnionOffset() + 16
	}
	return inputUnionOffset() + 12
}

func mouseRecord(x, y int, flags, data uint32) []byte {
	buffer := make([]byte, inputRecordSize())
	putUint32(buffer, 0, 0) // INPUT_MOUSE
	offset := inputUnionOffset()
	putUint32(buffer, offset, uint32(x))
	putUint32(buffer, offset+4, uint32(y))
	putUint32(buffer, offset+8, data)
	putUint32(buffer, offset+12, flags)
	putUint32(buffer, offset+16, 0)
	putUintptr(buffer, mouseExtraOffset(), 0)
	return buffer
}

func keyRecord(vk, scan uint16, flags uint32) []byte {
	buffer := make([]byte, inputRecordSize())
	putUint32(buffer, 0, 1) // INPUT_KEYBOARD
	offset := inputUnionOffset()
	putUint16(buffer, offset, vk)
	putUint16(buffer, offset+2, scan)
	putUint32(buffer, offset+4, flags)
	putUint32(buffer, offset+8, 0)
	putUintptr(buffer, keyExtraOffset(), 0)
	return buffer
}

func sendInputRecords(records [][]byte) *APIError {
	if len(records) == 0 {
		return nil
	}
	size := inputRecordSize()
	buffer := make([]byte, size*len(records))
	for index, record := range records {
		copy(buffer[index*size:], record)
	}
	inserted, _, err := procSendInput.Call(
		uintptr(len(records)),
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(size),
	)
	runtime.KeepAlive(buffer)
	if int(inserted) != len(records) {
		return apiError("input_failed", "SendInput inserted "+strconv.Itoa(int(inserted))+" of "+strconv.Itoa(len(records))+" events: "+err.Error(), 500)
	}
	return nil
}

func absoluteCoordinates(x, y int) (uint32, uint32) {
	left, top, width, height := virtualScreenOrigin()
	if x < left {
		x = left
	}
	if x > left+width-1 {
		x = left + width - 1
	}
	if y < top {
		y = top
	}
	if y > top+height-1 {
		y = top + height - 1
	}
	var nx, ny int
	if width > 1 {
		nx = (x - left) * 65535 / (width - 1)
	}
	if height > 1 {
		ny = (y - top) * 65535 / (height - 1)
	}
	return uint32(nx), uint32(ny)
}

func captureScreen(req ScreenRequest) (*ScreenResult, *APIError) {
	setDPIAware()
	left, top, width, height := virtualScreenOrigin()
	if int64(width)*int64(height) > 33<<20 {
		return nil, apiError("screen_too_large", "virtual desktop is too large to capture in one image", 400)
	}
	hdc, _, _ := procGetDC.Call(0)
	if hdc == 0 {
		return nil, apiError("screen_capture_failed", "GetDC failed", 500)
	}
	defer procReleaseDC.Call(0, hdc)
	memDC, _, _ := procCreateCompatibleDC.Call(hdc)
	if memDC == 0 {
		return nil, apiError("screen_capture_failed", "CreateCompatibleDC failed", 500)
	}
	defer procDeleteDC.Call(memDC)
	header := bitmapInfoHeader{
		Size:        uint32(unsafe.Sizeof(bitmapInfoHeader{})),
		Width:       int32(width),
		Height:      -int32(height), // top-down
		Planes:      1,
		BitCount:    32,
		Compression: 0,
		SizeImage:   uint32(width * height * 4),
	}
	// 直接创建 DIB 写入像素。GetDIBits 在 64 位进程中要求位图未选入 DC，且跨位数行为不一致，
	// 曾导致 x64 截屏返回 0 行。
	var bits unsafe.Pointer
	bitmap, _, _ := procCreateDIBSection.Call(
		hdc,
		uintptr(unsafe.Pointer(&header)),
		dibRGBColors,
		uintptr(unsafe.Pointer(&bits)),
		0,
		0,
	)
	if bitmap == 0 || bits == nil {
		return nil, apiError("screen_capture_failed", "CreateDIBSection failed", 500)
	}
	defer procDeleteObject.Call(bitmap)
	old, _, _ := procSelectObject.Call(memDC, bitmap)
	if old == 0 {
		return nil, apiError("screen_capture_failed", "SelectObject failed", 500)
	}
	ok, _, _ := procBitBlt.Call(memDC, 0, 0, uintptr(width), uintptr(height), hdc,
		uintptr(left), uintptr(top), srcCopy|captureBlt)
	if ok == 0 {
		ok, _, _ = procBitBlt.Call(memDC, 0, 0, uintptr(width), uintptr(height), hdc,
			uintptr(left), uintptr(top), srcCopy)
	}
	procSelectObject.Call(memDC, old)
	if ok == 0 {
		return nil, apiError("screen_capture_failed", "BitBlt failed", 500)
	}
	pixelLen := width * height * 4
	source := (*[1 << 30]byte)(bits)[:pixelLen:pixelLen]
	pixels := make([]byte, pixelLen)
	copy(pixels, source)
	runtime.KeepAlive(bitmap)

	img := image.NewRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		src := pixels[y*width*4 : (y+1)*width*4]
		dst := img.Pix[y*img.Stride : y*img.Stride+width*4]
		for x := 0; x < width; x++ {
			dst[x*4] = src[x*4+2]
			dst[x*4+1] = src[x*4+1]
			dst[x*4+2] = src[x*4]
			dst[x*4+3] = 255
		}
	}

	maxWidth := req.MaxWidth
	if maxWidth <= 0 {
		maxWidth = 1600
	}
	maxHeight := req.MaxHeight
	if maxHeight <= 0 {
		maxHeight = 1200
	}
	if width > maxWidth || height > maxHeight {
		scale := 1.0
		if width > maxWidth {
			scale = float64(maxWidth) / float64(width)
		}
		if height > maxHeight {
			candidate := float64(maxHeight) / float64(height)
			if candidate < scale {
				scale = candidate
			}
		}
		newWidth := int(float64(width) * scale)
		newHeight := int(float64(height) * scale)
		if newWidth < 1 {
			newWidth = 1
		}
		if newHeight < 1 {
			newHeight = 1
		}
		img = resizeNearest(img, newWidth, newHeight)
	}

	var encoded bytes.Buffer
	if err := png.Encode(&encoded, img); err != nil {
		return nil, apiError("screen_capture_failed", "PNG encoding failed: "+err.Error(), 500)
	}
	data := encoded.Bytes()
	return &ScreenResult{
		Width:     img.Bounds().Dx(),
		Height:    img.Bounds().Dy(),
		Format:    "png",
		PNGBase64: base64.StdEncoding.EncodeToString(data),
		Bytes:     len(data),
	}, nil
}

func resizeNearest(src *image.RGBA, width, height int) *image.RGBA {
	oldWidth := src.Bounds().Dx()
	oldHeight := src.Bounds().Dy()
	if oldWidth == width && oldHeight == height {
		return src
	}
	dst := image.NewRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		sy := y * oldHeight / height
		for x := 0; x < width; x++ {
			sx := x * oldWidth / width
			srcOffset := sy*src.Stride + sx*4
			dstOffset := y*dst.Stride + x*4
			copy(dst.Pix[dstOffset:dstOffset+4], src.Pix[srcOffset:srcOffset+4])
		}
	}
	return dst
}

func buttonFlags(button string, down bool) uint32 {
	switch strings.ToLower(strings.TrimSpace(button)) {
	case "right":
		if down {
			return mouseRightDown
		}
		return mouseRightUp
	case "middle":
		if down {
			return mouseMiddleDown
		}
		return mouseMiddleUp
	default:
		if down {
			return mouseLeftDown
		}
		return mouseLeftUp
	}
}

func moveRecord(x, y int) []byte {
	nx, ny := absoluteCoordinates(x, y)
	return mouseRecord(int(nx), int(ny), mouseMove|mouseAbsolute|mouseVirtualDesk, 0)
}

func injectMouse(req MouseRequest) (*MouseResult, *APIError) {
	setDPIAware()
	action := strings.ToLower(strings.TrimSpace(req.Action))
	if action == "" {
		action = "click"
	}
	switch action {
	case "move":
		if err := sendInputRecords([][]byte{moveRecord(req.X, req.Y)}); err != nil {
			return nil, err
		}
	case "click", "down", "up":
		nx, ny := absoluteCoordinates(req.X, req.Y)
		flags := uint32(mouseAbsolute | mouseVirtualDesk)
		if action == "click" {
			if err := sendInputRecords([][]byte{
				mouseRecord(int(nx), int(ny), flags|buttonFlags(req.Button, true), 0),
				mouseRecord(int(nx), int(ny), flags|buttonFlags(req.Button, false), 0),
			}); err != nil {
				return nil, err
			}
		} else {
			if err := sendInputRecords([][]byte{
				mouseRecord(int(nx), int(ny), flags|buttonFlags(req.Button, action == "down"), 0),
			}); err != nil {
				return nil, err
			}
		}
	case "scroll":
		nx, ny := absoluteCoordinates(req.X, req.Y)
		delta := req.Delta
		if delta == 0 {
			delta = 1
		}
		flags := uint32(mouseWheel | mouseAbsolute | mouseVirtualDesk)
		if err := sendInputRecords([][]byte{
			mouseRecord(int(nx), int(ny), flags, uint32(int32(delta*120))),
		}); err != nil {
			return nil, err
		}
	case "drag":
		duration := req.DurationMS
		if duration <= 0 {
			duration = 300
		}
		if duration > 10000 {
			duration = 10000
		}
		records := [][]byte{moveRecord(req.X1, req.Y1)}
		nx, ny := absoluteCoordinates(req.X1, req.Y1)
		records = append(records, mouseRecord(int(nx), int(ny), mouseAbsolute|mouseVirtualDesk|buttonFlags(req.Button, true), 0))
		if err := sendInputRecords(records); err != nil {
			return nil, err
		}
		steps := 20
		for step := 1; step <= steps; step++ {
			x := req.X1 + (req.X2-req.X1)*step/steps
			y := req.Y1 + (req.Y2-req.Y1)*step/steps
			if err := sendInputRecords([][]byte{moveRecord(x, y)}); err != nil {
				return nil, err
			}
			time.Sleep(time.Duration(duration/steps) * time.Millisecond)
		}
		ex, ey := absoluteCoordinates(req.X2, req.Y2)
		if err := sendInputRecords([][]byte{
			mouseRecord(int(ex), int(ey), mouseAbsolute|mouseVirtualDesk|buttonFlags(req.Button, false), 0),
		}); err != nil {
			return nil, err
		}
	default:
		return nil, invalidArg("action must be move, click, down, up, scroll or drag")
	}
	return &MouseResult{Action: action, X: req.X, Y: req.Y}, nil
}

func injectKeyboard(req KeyboardRequest) (*KeyboardResult, *APIError) {
	action := strings.ToLower(strings.TrimSpace(req.Action))
	if action == "" {
		if req.Text != "" {
			action = "type"
		} else {
			action = "key"
		}
	}
	switch action {
	case "type":
		if req.Text == "" {
			return nil, invalidArg("text is required for type")
		}
		units := syscall.StringToUTF16(req.Text)
		if len(units) > 0 && units[len(units)-1] == 0 {
			units = units[:len(units)-1]
		}
		records := make([][]byte, 0, len(units)*2)
		for _, unit := range units {
			records = append(records, keyRecord(0, unit, keyEventUnicode))
			records = append(records, keyRecord(0, unit, keyEventUnicode|keyEventKeyUp))
		}
		if err := sendInputRecords(records); err != nil {
			return nil, err
		}
		return &KeyboardResult{Action: "type", Characters: utf8.RuneCountInString(req.Text)}, nil
	case "key":
		modifiers, key, apiErr := parseKeyCombo(req.Key)
		if apiErr != nil {
			return nil, apiErr
		}
		records := make([][]byte, 0, len(modifiers)*2+2)
		for _, modifier := range modifiers {
			records = append(records, keyRecord(modifier, 0, 0))
		}
		records = append(records, keyRecord(key, 0, 0))
		records = append(records, keyRecord(key, 0, keyEventKeyUp))
		for index := len(modifiers) - 1; index >= 0; index-- {
			records = append(records, keyRecord(modifiers[index], 0, keyEventKeyUp))
		}
		if err := sendInputRecords(records); err != nil {
			return nil, err
		}
		return &KeyboardResult{Action: "key", Characters: 1}, nil
	default:
		return nil, invalidArg("action must be type or key")
	}
}

func parseKeyCombo(value string) ([]uint16, uint16, *APIError) {
	parts := strings.Split(value, "+")
	if len(parts) == 0 || strings.TrimSpace(value) == "" {
		return nil, 0, invalidArg("key is required")
	}
	modifierSet := map[uint16]bool{}
	for index := 0; index < len(parts)-1; index++ {
		modifier, ok := modifierVirtualKey(parts[index])
		if !ok {
			return nil, 0, invalidArg("unknown modifier: " + parts[index])
		}
		modifierSet[modifier] = true
	}
	keyToken := strings.TrimSpace(parts[len(parts)-1])
	if keyToken == "" {
		return nil, 0, invalidArg("key is required")
	}
	key, extraModifiers, apiErr := parseVirtualKey(keyToken)
	if apiErr != nil {
		return nil, 0, apiErr
	}
	for _, modifier := range extraModifiers {
		modifierSet[modifier] = true
	}
	order := []uint16{0x11, 0x12, 0x10, 0x5B}
	modifiers := make([]uint16, 0, len(modifierSet))
	for _, modifier := range order {
		if modifierSet[modifier] {
			modifiers = append(modifiers, modifier)
		}
	}
	return modifiers, key, nil
}

func modifierVirtualKey(token string) (uint16, bool) {
	switch strings.ToUpper(strings.TrimSpace(token)) {
	case "CTRL", "CONTROL":
		return 0x11, true
	case "ALT", "MENU":
		return 0x12, true
	case "SHIFT":
		return 0x10, true
	case "WIN", "LWIN", "WINDOWS":
		return 0x5B, true
	default:
		return 0, false
	}
}

func parseVirtualKey(token string) (uint16, []uint16, *APIError) {
	upper := strings.ToUpper(strings.TrimSpace(token))
	named := map[string]uint16{
		"ENTER": 0x0D, "RETURN": 0x0D, "TAB": 0x09, "ESC": 0x1B, "ESCAPE": 0x1B,
		"SPACE": 0x20, "BACKSPACE": 0x08, "DELETE": 0x2E, "DEL": 0x2E, "INSERT": 0x2D, "INS": 0x2D,
		"HOME": 0x24, "END": 0x23, "PAGEUP": 0x21, "PGUP": 0x21, "PAGEDOWN": 0x22, "PGDN": 0x22,
		"UP": 0x26, "DOWN": 0x28, "LEFT": 0x25, "RIGHT": 0x27,
		"CAPSLOCK": 0x14, "NUMLOCK": 0x90, "SCROLLLOCK": 0x91,
		"PRINTSCREEN": 0x2C, "PAUSE": 0x13,
	}
	if vk, ok := named[upper]; ok {
		return vk, nil, nil
	}
	if strings.HasPrefix(upper, "F") && len(upper) <= 3 {
		if number, err := strconv.Atoi(upper[1:]); err == nil && number >= 1 && number <= 24 {
			return uint16(0x70 + number - 1), nil, nil
		}
	}
	if len(upper) == 1 {
		r := rune(upper[0])
		if r >= 'A' && r <= 'Z' {
			return uint16(r), nil, nil
		}
		if r >= '0' && r <= '9' {
			return uint16(r), nil, nil
		}
	}
	runes := []rune(token)
	if len(runes) != 1 {
		return 0, nil, invalidArg("unknown key: " + token)
	}
	scan, _, _ := procVkKeyScanW.Call(uintptr(runes[0]))
	if scan == 0xFFFF {
		return 0, nil, invalidArg("cannot map key: " + token)
	}
	vk := uint16(scan & 0xFF)
	shiftState := byte((scan >> 8) & 0xFF)
	var modifiers []uint16
	if shiftState&1 != 0 {
		modifiers = append(modifiers, 0x10)
	}
	if shiftState&2 != 0 {
		modifiers = append(modifiers, 0x11)
	}
	if shiftState&4 != 0 {
		modifiers = append(modifiers, 0x12)
	}
	return vk, modifiers, nil
}

func listWindows() (*WindowListResult, *APIError) {
	setDPIAware()
	var windows []WindowInfo
	callback := syscall.NewCallback(func(hwnd uintptr, lparam uintptr) uintptr {
		visible, _, _ := procIsWindowVisible.Call(hwnd)
		if visible == 0 {
			return 1
		}
		length, _, _ := procGetWindowTextLengthW.Call(hwnd)
		if length == 0 {
			return 1
		}
		buffer := make([]uint16, int(length)+1)
		procGetWindowTextW.Call(hwnd, uintptr(unsafe.Pointer(&buffer[0])), uintptr(len(buffer)))
		title := strings.TrimSpace(syscall.UTF16ToString(buffer))
		if title == "" {
			return 1
		}
		var pid uint32
		procGetWindowThreadPID.Call(hwnd, uintptr(unsafe.Pointer(&pid)))
		var bounds rect
		procGetWindowRect.Call(hwnd, uintptr(unsafe.Pointer(&bounds)))
		minimized, _, _ := procIsIconic.Call(hwnd)
		windows = append(windows, WindowInfo{
			HWND:      fmt.Sprintf("0x%x", hwnd),
			PID:       pid,
			Title:     title,
			Minimized: minimized != 0,
			Rect: RectInfo{
				Left:   bounds.Left,
				Top:    bounds.Top,
				Right:  bounds.Right,
				Bottom: bounds.Bottom,
			},
		})
		if len(windows) >= 2000 {
			return 0
		}
		return 1
	})
	procEnumWindows.Call(callback, 0)
	runtime.KeepAlive(callback)
	sort.Slice(windows, func(i, j int) bool {
		if windows[i].PID == windows[j].PID {
			return windows[i].Title < windows[j].Title
		}
		return windows[i].PID < windows[j].PID
	})
	return &WindowListResult{Windows: windows}, nil
}

func focusWindow(req WindowFocusRequest) (*WindowFocusResult, *APIError) {
	hwnd, err := parseHWND(req.HWND)
	if err != nil {
		return nil, invalidArg("invalid hwnd: " + req.HWND)
	}
	valid, _, _ := procIsWindow.Call(hwnd)
	if valid == 0 {
		return nil, apiError("not_found", "window not found: "+req.HWND, 404)
	}
	if minimized, _, _ := procIsIconic.Call(hwnd); minimized != 0 {
		procShowWindow.Call(hwnd, swRestore)
	}
	focused, _, _ := procSetForegroundWindow.Call(hwnd)
	if focused == 0 {
		return nil, apiError("focus_failed", "SetForegroundWindow failed", 409)
	}
	return &WindowFocusResult{HWND: fmt.Sprintf("0x%x", hwnd), Focused: true}, nil
}

func parseHWND(value string) (uintptr, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0, fmt.Errorf("empty hwnd")
	}
	var parsed uint64
	var err error
	if strings.HasPrefix(strings.ToLower(value), "0x") {
		parsed, err = strconv.ParseUint(value[2:], 16, 64)
	} else {
		parsed, err = strconv.ParseUint(value, 10, 64)
	}
	if err != nil {
		return 0, err
	}
	return uintptr(parsed), nil
}
