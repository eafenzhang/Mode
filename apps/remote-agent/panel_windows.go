//go:build windows
// +build windows

package main

import (
	"fmt"
	"image"
	"math/rand"
	"os"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode/utf16"
	"unsafe"
)

const (
	wmDestroy       = 0x0002
	wmPaint         = 0x000F
	wmClose         = 0x0010
	wmEraseBkgnd    = 0x0014
	wmGetMinMaxInfo = 0x0024
	wmTimer         = 0x0113
	wmSysCommand    = 0x0112
	wmLButtonDown   = 0x0201
	wmLButtonUp     = 0x0202
	wmMouseWheel    = 0x020A
	wmApp           = 0x8000
	wmTrayCallback  = wmApp + 1
	wmPanelShow     = wmApp + 2
	wmPanelRedraw   = wmApp + 3
	wmPanelQuit     = wmApp + 4
	wmPanelSettings = wmApp + 5
	wmPanelPair     = wmApp + 6

	wsOverlapped  = 0x00000000
	wsCaption     = 0x00C00000
	wsSysMenu     = 0x00080000
	wsMinimizeBox = 0x00020000
	wsExAppWindow = 0x00040000

	swHide       = 0
	swShowNormal = 1

	dtLeft       = 0x0000
	dtTop        = 0x0000
	dtCenter     = 0x0001
	dtVCenter    = 0x0002
	dtWordBreak  = 0x0010
	dtSingleLine = 0x0100
	dtCalcrect   = 0x0400
	dtNoPrefix   = 0x2000
	psSolid      = 0

	animationTimerID    = 1
	animationIntervalMS = 50

	transparentBkMode = 1
	whiteBkBrush      = 0

	nimAdd    = 0
	nimDelete = 2
	nifMsg    = 0x00000001
	nifIcon   = 0x00000002
	nifTip    = 0x00000004

	mfString    = 0x00000000
	mfSeparator = 0x00000800

	idcArrow       = 32512
	idiApplication = 32512

	fwNormal = 400
	fwBold   = 700

	defaultCharset   = 1
	cleartypeQuality = 5

	// 系统菜单自定义命令 ID；低 4 位必须为 0（Windows 内部会使用）。
	sysMenuSettings = 0xE000
	sysMenuPair     = 0xE010
	sysMenuExit     = 0xE020
)

var (
	procRegisterClassExW      = user32.NewProc("RegisterClassExW")
	procCreateWindowExW       = user32.NewProc("CreateWindowExW")
	procDefWindowProcW        = user32.NewProc("DefWindowProcW")
	procDestroyWindow         = user32.NewProc("DestroyWindow")
	procPostMessageW          = user32.NewProc("PostMessageW")
	procGetClassNameW         = user32.NewProc("GetClassNameW")
	procGetMessageW           = user32.NewProc("GetMessageW")
	procTranslateMessage      = user32.NewProc("TranslateMessage")
	procDispatchMessageW      = user32.NewProc("DispatchMessageW")
	procPostQuitMessage       = user32.NewProc("PostQuitMessage")
	procAdjustWindowRectEx    = user32.NewProc("AdjustWindowRectEx")
	procBeginPaint            = user32.NewProc("BeginPaint")
	procEndPaint              = user32.NewProc("EndPaint")
	procGetClientRect         = user32.NewProc("GetClientRect")
	procDrawTextW             = user32.NewProc("DrawTextW")
	procFillRect              = user32.NewProc("FillRect")
	procLoadCursorW           = user32.NewProc("LoadCursorW")
	procLoadIconW             = user32.NewProc("LoadIconW")
	procGetSystemMenu         = user32.NewProc("GetSystemMenu")
	procAppendMenuW           = user32.NewProc("AppendMenuW")
	procInvalidateRect        = user32.NewProc("InvalidateRect")
	procUpdateWindow          = user32.NewProc("UpdateWindow")
	procSystemParametersInfoW = user32.NewProc("SystemParametersInfoW")
	procSetTimer              = user32.NewProc("SetTimer")
	procKillTimer             = user32.NewProc("KillTimer")

	procSetTextColor          = gdi32.NewProc("SetTextColor")
	procSetBkMode             = gdi32.NewProc("SetBkMode")
	procGetStockObject        = gdi32.NewProc("GetStockObject")
	procCreateSolidBrush      = gdi32.NewProc("CreateSolidBrush")
	procCreateFontW           = gdi32.NewProc("CreateFontW")
	procTextOutW              = gdi32.NewProc("TextOutW")
	procStretchDIBits         = gdi32.NewProc("StretchDIBits")
	procGetDeviceCaps         = gdi32.NewProc("GetDeviceCaps")
	procCreateCompatDC        = gdi32.NewProc("CreateCompatibleDC")
	procCreateCompatBmp       = gdi32.NewProc("CreateCompatibleBitmap")
	procRoundRect             = gdi32.NewProc("RoundRect")
	procCreatePen             = gdi32.NewProc("CreatePen")
	procSaveDC                = gdi32.NewProc("SaveDC")
	procRestoreDC             = gdi32.NewProc("RestoreDC")
	procIntersectClipRect     = gdi32.NewProc("IntersectClipRect")
	procGetTextExtentPoint32W = gdi32.NewProc("GetTextExtentPoint32W")

	procFreeConsole = kernel32.NewProc("FreeConsole")

	shell32         = syscall.NewLazyDLL("shell32.dll")
	procShellNotify = shell32.NewProc("Shell_NotifyIconW")
)

type uiPoint struct {
	X int32
	Y int32
}

type uiRect struct {
	Left   int32
	Top    int32
	Right  int32
	Bottom int32
}

type uiMsg struct {
	HWnd     uintptr
	Message  uint32
	WParam   uintptr
	LParam   uintptr
	Time     uint32
	Pt       uiPoint
	LPrivate uint32
}

type wndClassExW struct {
	CbSize        uint32
	Style         uint32
	LpfnWndProc   uintptr
	CbClsExtra    int32
	CbWndExtra    int32
	HInstance     uintptr
	HIcon         uintptr
	HCursor       uintptr
	HbrBackground uintptr
	LpszMenuName  *uint16
	LpszClassName *uint16
	HIconSm       uintptr
}

type paintStruct struct {
	Hdc         uintptr
	FErase      int32
	RcPaint     uiRect
	FRestore    int32
	FIncUpdate  int32
	RGBReserved [32]byte
}

type minMaxInfo struct {
	Reserved     uiPoint
	MaxSize      uiPoint
	MaxPosition  uiPoint
	MinTrackSize uiPoint
	MaxTrackSize uiPoint
}

// notifyIconDataW 使用 NOTIFYICONDATAW 的 V2 布局（XP 支持，且后续系统兼容）。
type notifyIconDataW struct {
	CbSize           uint32
	HWnd             uintptr
	UID              uint32
	UFlags           uint32
	UCallbackMessage uint32
	HIcon            uintptr
	SzTip            [128]uint16
	DwState          uint32
	DwStateMask      uint32
	SzInfo           [256]uint16
	UTimeout         uint32
	SzInfoTitle      [64]uint16
	DwInfoFlags      uint32
}

type agentUIHandle struct {
	cfg             *Config
	panel           *panelController
	requestShutdown func()

	mu          sync.Mutex
	hwnd        uintptr
	dpi         int32
	width       int32
	height      int32
	outerWidth  int32
	outerHeight int32
	quitting    bool
	done        chan struct{}
	ready       chan error

	titleFont uintptr
	textFont  uintptr
	rainFont  uintptr
	tray      bool

	// 声明式 UI 块的渲染状态（仅 UI 线程访问）。
	boldFont      uintptr
	smallFont     uintptr
	statFont      uintptr
	badgeFont     uintptr
	footerFont    uintptr
	scrollY       int32 // 结构化视图当前滚动偏移（物理像素）
	footerRect    uiRect
	footerEnabled bool
	footerPressed bool
	lastStamp     time.Time // 上一次渲染的内容时间戳，内容变化时重置滚动

	memDC        uintptr
	memBitmap    uintptr
	memOldBitmap uintptr
	memWidth     int32
	memHeight    int32

	anim *matrixAnimation

	imageMu     sync.Mutex
	imageRef    *image.RGBA
	imagePixels []byte
	imageWidth  int
	imageHeight int
}

type rainColumn struct {
	x      int32
	y      float64
	speed  float64
	length int
	glyphs []uint16
}

type matrixAnimation struct {
	width      int32
	height     int32
	spacing    int32
	lineHeight int32
	columns    []rainColumn
	glyphPool  []uint16
	phase      float64
	tick       int
	rng        *rand.Rand
}

var (
	agentUIMutex    sync.Mutex
	agentUIWindow   uintptr
	agentUIInstance *agentUIHandle
	agentUIClassPtr *uint16
	agentUIProcPtr  uintptr
	agentUIIcon     uintptr
)

func panelClassName(cfg *Config) string {
	id := ""
	if cfg != nil {
		id = strings.TrimSpace(cfg.AgentID)
	}
	if id == "" {
		return "ModeAgentPanel"
	}
	return "ModeAgentPanel_" + id
}

func agentPanelVisible() bool {
	agentUIMutex.Lock()
	hwnd := agentUIWindow
	agentUIMutex.Unlock()
	if hwnd == 0 {
		return false
	}
	visible, _, _ := procIsWindowVisible.Call(hwnd)
	return visible != 0
}

func panelSupported() bool {
	return true
}

func notifyPanelChanged() {
	agentUIMutex.Lock()
	hwnd := agentUIWindow
	agentUIMutex.Unlock()
	if hwnd != 0 {
		procPostMessageW.Call(hwnd, wmPanelRedraw, 0, 0)
	}
}

func showAgentPanel() bool {
	agentUIMutex.Lock()
	hwnd := agentUIWindow
	agentUIMutex.Unlock()
	if hwnd == 0 {
		return false
	}
	procPostMessageW.Call(hwnd, wmPanelShow, 0, 0)
	return true
}

var panelFinderState struct {
	mu     sync.Mutex
	target string
	found  uintptr
}

var panelFinderCallback = syscall.NewCallback(panelFinderProc)

func panelFinderProc(hwnd, lparam uintptr) uintptr {
	var buffer [256]uint16
	length, _, _ := procGetClassNameW.Call(
		hwnd,
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(len(buffer)),
	)
	if int(length) <= 0 {
		return 1
	}
	name := syscall.UTF16ToString(buffer[:int(length)])
	panelFinderState.mu.Lock()
	match := name == panelFinderState.target
	if match {
		panelFinderState.found = hwnd
	}
	panelFinderState.mu.Unlock()
	if match {
		return 0
	}
	return 1
}

// openAgentPanelWindow 由第二个进程调用：按类名找到已运行 agent 的面板并请求显示。
// 这里不用 FindWindow：部分 Windows 环境下它对 RegisterClassExW 注册的类返回 0，
// 而 EnumWindows + GetClassNameW 能稳定找到同一窗口。
func openAgentPanelWindow(cfg *Config) bool {
	panelFinderState.mu.Lock()
	panelFinderState.target = panelClassName(cfg)
	panelFinderState.found = 0
	panelFinderState.mu.Unlock()
	procEnumWindows.Call(panelFinderCallback, 0)
	panelFinderState.mu.Lock()
	hwnd := panelFinderState.found
	panelFinderState.mu.Unlock()
	if hwnd == 0 {
		return false
	}
	procPostMessageW.Call(hwnd, wmPanelShow, 0, 0)
	return true
}

func startAgentUI(cfg *Config, panel *panelController, requestShutdown func()) (*agentUIHandle, error) {
	ui := &agentUIHandle{
		cfg:             cfg,
		panel:           panel,
		requestShutdown: requestShutdown,
		done:            make(chan struct{}),
		ready:           make(chan error, 1),
	}
	agentUIMutex.Lock()
	if agentUIInstance != nil {
		agentUIMutex.Unlock()
		return nil, fmt.Errorf("panel UI is already running")
	}
	agentUIInstance = ui
	agentUIMutex.Unlock()
	go ui.run()
	select {
	case err := <-ui.ready:
		if err != nil {
			agentUIMutex.Lock()
			agentUIInstance = nil
			agentUIMutex.Unlock()
			return nil, err
		}
		return ui, nil
	case <-time.After(5 * time.Second):
		return nil, fmt.Errorf("timed out creating the panel window")
	}
}

func (ui *agentUIHandle) stop() {
	ui.mu.Lock()
	if ui.quitting {
		ui.mu.Unlock()
		<-ui.done
		return
	}
	ui.quitting = true
	hwnd := ui.hwnd
	ui.mu.Unlock()
	if hwnd != 0 {
		procPostMessageW.Call(hwnd, wmPanelQuit, 0, 0)
	}
	select {
	case <-ui.done:
	case <-time.After(3 * time.Second):
	}
}

func (ui *agentUIHandle) show() {
	if ui == nil {
		return
	}
	showAgentPanel()
}

func (ui *agentUIHandle) run() {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(ui.done)

	procSetProcessDPIAware.Call()
	ui.dpi = systemDPI()
	ui.width = scaleDIP(panelWidthDIP, ui.dpi)
	ui.height = scaleDIP(panelHeightDIP, ui.dpi)

	className := panelClassName(ui.cfg)
	classNamePtr, err := syscall.UTF16PtrFromString(className)
	if err != nil {
		ui.ready <- err
		return
	}
	agentUIClassPtr = classNamePtr
	agentUIIcon = loadAgentIcon()
	proc := syscall.NewCallback(panelWindowProc)
	agentUIProcPtr = proc
	instance, _, _ := procGetModuleHandleW.Call(0)
	cursor, _, _ := procLoadCursorW.Call(0, idcArrow)
	whiteBrush, _, _ := procGetStockObject.Call(whiteBkBrush)
	wc := wndClassExW{
		CbSize:        uint32(unsafe.Sizeof(wndClassExW{})),
		Style:         0x0003, // CS_HREDRAW | CS_VREDRAW
		LpfnWndProc:   proc,
		HInstance:     instance,
		HIcon:         agentUIIcon,
		HCursor:       cursor,
		HbrBackground: whiteBrush,
		LpszClassName: classNamePtr,
		HIconSm:       agentUIIcon,
	}
	if atom, _, callErr := procRegisterClassExW.Call(uintptr(unsafe.Pointer(&wc))); atom == 0 {
		ui.ready <- fmt.Errorf("register panel window class: %v", callErr)
		return
	}
	style := uint32(wsOverlapped | wsCaption | wsSysMenu | wsMinimizeBox)
	exStyle := uint32(wsExAppWindow)
	windowRect := uiRect{Right: ui.width, Bottom: ui.height}
	procAdjustWindowRectEx.Call(
		uintptr(unsafe.Pointer(&windowRect)),
		uintptr(style),
		0,
		uintptr(exStyle),
	)
	winW := windowRect.Right - windowRect.Left
	winH := windowRect.Bottom - windowRect.Top
	ui.mu.Lock()
	ui.outerWidth = winW
	ui.outerHeight = winH
	ui.mu.Unlock()
	x, y := centeredWindowPosition(winW, winH)
	titlePtr, _ := syscall.UTF16PtrFromString("Mode Go")
	hwnd, _, callErr := procCreateWindowExW.Call(
		uintptr(exStyle),
		uintptr(unsafe.Pointer(classNamePtr)),
		uintptr(unsafe.Pointer(titlePtr)),
		uintptr(style),
		uintptr(x),
		uintptr(y),
		uintptr(winW),
		uintptr(winH),
		0,
		0,
		instance,
		0,
	)
	runtime.KeepAlive(classNamePtr)
	runtime.KeepAlive(titlePtr)
	if hwnd == 0 {
		ui.ready <- fmt.Errorf("create panel window: %v", callErr)
		return
	}
	ui.mu.Lock()
	ui.hwnd = hwnd
	ui.mu.Unlock()
	ui.titleFont = createUIFont(scaleDIP(20, ui.dpi), fwBold)
	ui.textFont = createUIFont(scaleDIP(14, ui.dpi), fwNormal)
	ui.boldFont = createUIFont(scaleDIP(15, ui.dpi), fwBold)
	ui.smallFont = createUIFont(scaleDIP(13, ui.dpi), fwNormal)
	ui.statFont = createUIFont(scaleDIP(28, ui.dpi), fwBold)
	ui.badgeFont = createUIFont(scaleDIP(12, ui.dpi), fwNormal)
	ui.footerFont = createUIFont(scaleDIP(15, ui.dpi), fwBold)
	ui.rainFont = createFontWithFace("Consolas", scaleDIP(14, ui.dpi), fwNormal)
	if ui.rainFont == 0 {
		ui.rainFont = createFontWithFace("Courier New", scaleDIP(14, ui.dpi), fwNormal)
	}
	procSetTimer.Call(hwnd, animationTimerID, animationIntervalMS, 0)
	installSystemMenu(hwnd)
	agentUIMutex.Lock()
	agentUIWindow = hwnd
	agentUIMutex.Unlock()
	if ui.cfg != nil && ui.cfg.Panel.ShowTray {
		ui.addTrayIcon()
	}
	hideConsoleForPanel()
	ui.ready <- nil

	var message uiMsg
	for {
		result, _, _ := procGetMessageW.Call(uintptr(unsafe.Pointer(&message)), 0, 0, 0)
		if int32(result) <= 0 {
			break
		}
		procTranslateMessage.Call(uintptr(unsafe.Pointer(&message)))
		procDispatchMessageW.Call(uintptr(unsafe.Pointer(&message)))
	}
	ui.removeTrayIcon()
	ui.releaseBackBuffer()
	ui.deleteFonts()
	agentUIMutex.Lock()
	if agentUIWindow == hwnd {
		agentUIWindow = 0
	}
	if agentUIInstance == ui {
		agentUIInstance = nil
	}
	agentUIMutex.Unlock()
}

func panelWindowProc(hwnd, message, wParam, lParam uintptr) uintptr {
	ui := currentUI()
	switch message {
	case wmGetMinMaxInfo:
		if ui != nil && lParam != 0 {
			info := (*minMaxInfo)(unsafe.Pointer(lParam))
			width, height := ui.trackSize()
			info.MinTrackSize = uiPoint{X: width, Y: height}
			info.MaxTrackSize = uiPoint{X: width, Y: height}
			return 0
		}
	case wmPaint:
		if ui != nil {
			ui.paint()
			return 0
		}
	case wmEraseBkgnd:
		return 1
	case wmClose:
		if ui != nil && !ui.isQuitting() {
			procShowWindow.Call(hwnd, swHide)
			return 0
		}
		return 0
	case wmSysCommand:
		switch wParam {
		case sysMenuSettings:
			if ui != nil {
				ui.showSettings()
				return 0
			}
		case sysMenuPair:
			if ui != nil {
				ui.showPairing()
				return 0
			}
		case sysMenuExit:
			if ui != nil {
				if ui.requestShutdown != nil {
					ui.requestShutdown()
				} else {
					procPostMessageW.Call(hwnd, wmPanelQuit, 0, 0)
				}
				return 0
			}
		}
	case wmTrayCallback:
		if ui != nil {
			event := uint32(lParam & 0xFFFF)
			switch event {
			case 0x0203: // WM_LBUTTONDBLCLK
				ui.showPanel()
			}
			return 0
		}
	case wmPanelShow:
		if ui != nil {
			ui.showPanel()
			return 0
		}
	case wmPanelRedraw:
		if ui != nil {
			ui.invalidatePanel()
			return 0
		}
	case wmPanelSettings:
		if ui != nil {
			ui.showSettings()
			return 0
		}
	case wmPanelPair:
		if ui != nil {
			ui.showPairing()
			return 0
		}
	case wmMouseWheel:
		if ui != nil {
			// GET_WHEEL_DELTA_WPARAM：wParam 高 16 位是有符号 delta，向上滚为正。
			delta := int32(int16((wParam >> 16) & 0xFFFF))
			if delta != 0 && ui.scrollBy(-delta) {
				return 0
			}
		}
	case wmLButtonDown:
		if ui != nil && lParam != 0 {
			x := int32(int16(lParam & 0xFFFF))
			y := int32(int16((lParam >> 16) & 0xFFFF))
			if ui.beginFooterPress(x, y) {
				return 0
			}
		}
	case wmLButtonUp:
		if ui != nil && lParam != 0 {
			x := int32(int16(lParam & 0xFFFF))
			y := int32(int16((lParam >> 16) & 0xFFFF))
			if ui.completeFooterPress(x, y) {
				return 0
			}
		}
	case wmTimer:
		if ui != nil && wParam == animationTimerID {
			ui.tickAnimation()
			return 0
		}
	case wmPanelQuit:
		procDestroyWindow.Call(hwnd)
		return 0
	case wmDestroy:
		procKillTimer.Call(hwnd, animationTimerID)
		procPostQuitMessage.Call(0)
		return 0
	}
	result, _, _ := procDefWindowProcW.Call(hwnd, message, wParam, lParam)
	return result
}

func currentUI() *agentUIHandle {
	agentUIMutex.Lock()
	defer agentUIMutex.Unlock()
	return agentUIInstance
}

func (ui *agentUIHandle) trackSize() (int32, int32) {
	ui.mu.Lock()
	defer ui.mu.Unlock()
	return ui.outerWidth, ui.outerHeight
}

func (ui *agentUIHandle) isQuitting() bool {
	ui.mu.Lock()
	defer ui.mu.Unlock()
	return ui.quitting
}

func (ui *agentUIHandle) showPanel() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	quitting := ui.quitting
	ui.mu.Unlock()
	if hwnd == 0 || quitting {
		return
	}
	procShowWindow.Call(hwnd, swShowNormal)
	// ShowWindow 的第一次调用可能被进程启动时的 SW_HIDE 吞掉；SetWindowPos 确保可见。
	procSetWindowPos.Call(
		hwnd,
		0, // HWND_TOP
		0,
		0,
		0,
		0,
		0x0001|0x0002|0x0004|0x0040, // NOSIZE | NOMOVE | NOZORDER | SHOWWINDOW
	)
	procSetForegroundWindow.Call(hwnd)
	procInvalidateRect.Call(hwnd, 0, 1)
	procUpdateWindow.Call(hwnd)
}

func (ui *agentUIHandle) invalidatePanel() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	ui.mu.Unlock()
	if hwnd == 0 {
		return
	}
	procInvalidateRect.Call(hwnd, 0, 1)
	procUpdateWindow.Call(hwnd)
}

func (ui *agentUIHandle) paint() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	ui.mu.Unlock()
	if hwnd == 0 {
		return
	}
	var ps paintStruct
	hdc, _, _ := procBeginPaint.Call(hwnd, uintptr(unsafe.Pointer(&ps)))
	if hdc == 0 {
		return
	}
	defer procEndPaint.Call(hwnd, uintptr(unsafe.Pointer(&ps)))
	var client uiRect
	procGetClientRect.Call(hwnd, uintptr(unsafe.Pointer(&client)))
	width := client.Right - client.Left
	height := client.Bottom - client.Top
	if width <= 0 || height <= 0 {
		return
	}
	ui.ensureBackBuffer(hdc, width, height)
	dc := hdc
	if ui.memDC != 0 {
		dc = ui.memDC
	}
	snapshot := ui.panel.snapshot()
	if panelSnapshotBlank(snapshot) {
		ui.drawAnimation(dc, width, height)
	} else {
		ui.drawContent(dc, snapshot, width, height)
	}
	if dc != hdc {
		procBitBlt.Call(
			hdc,
			0,
			0,
			uintptr(width),
			uintptr(height),
			dc,
			0,
			0,
			srcCopy,
		)
	}
}

func panelSnapshotBlank(snapshot PanelSnapshot) bool {
	return snapshot.Title == "" && snapshot.Text == "" && !snapshot.HasImage && len(snapshot.Blocks) == 0
}

func (ui *agentUIHandle) ensureBackBuffer(hdc uintptr, width, height int32) {
	if ui.memDC != 0 && ui.memWidth == width && ui.memHeight == height {
		return
	}
	ui.releaseBackBuffer()
	memDC, _, _ := procCreateCompatDC.Call(hdc)
	if memDC == 0 {
		return
	}
	bitmap, _, _ := procCreateCompatBmp.Call(hdc, uintptr(width), uintptr(height))
	if bitmap == 0 {
		procDeleteDC.Call(memDC)
		return
	}
	old, _, _ := procSelectObject.Call(memDC, bitmap)
	ui.memDC = memDC
	ui.memBitmap = bitmap
	ui.memOldBitmap = old
	ui.memWidth = width
	ui.memHeight = height
}

func (ui *agentUIHandle) releaseBackBuffer() {
	if ui.memDC != 0 {
		if ui.memOldBitmap != 0 {
			procSelectObject.Call(ui.memDC, ui.memOldBitmap)
		}
		if ui.memBitmap != 0 {
			procDeleteObject.Call(ui.memBitmap)
		}
		procDeleteDC.Call(ui.memDC)
	}
	ui.memDC = 0
	ui.memBitmap = 0
	ui.memOldBitmap = 0
	ui.memWidth = 0
	ui.memHeight = 0
}

func (ui *agentUIHandle) drawContent(dc uintptr, snapshot PanelSnapshot, width, height int32) {
	if len(snapshot.Blocks) > 0 {
		ui.drawBlocks(dc, snapshot, width, height)
		return
	}
	brush, _, _ := procCreateSolidBrush.Call(uintptr(rgb(255, 255, 255)))
	rect := uiRect{Right: width, Bottom: height}
	procFillRect.Call(dc, uintptr(unsafe.Pointer(&rect)), brush)
	procDeleteObject.Call(brush)

	padding := scaleDIP(16, ui.dpi)
	cursorY := padding
	if snapshot.Title != "" {
		font := ui.titleFont
		old := uintptr(0)
		if font != 0 {
			old, _, _ = procSelectObject.Call(dc, font)
		}
		procSetTextColor.Call(dc, uintptr(rgb(30, 30, 30)))
		procSetBkMode.Call(dc, transparentBkMode)
		textPtr, _ := syscall.UTF16PtrFromString(snapshot.Title)
		titleRect := uiRect{Left: padding, Top: cursorY, Right: width - padding, Bottom: height - padding}
		procDrawTextW.Call(dc, uintptr(unsafe.Pointer(textPtr)), uintptr(0xFFFFFFFF), uintptr(unsafe.Pointer(&titleRect)), uintptr(dtLeft|dtTop|dtWordBreak|dtNoPrefix))
		runtime.KeepAlive(textPtr)
		if old != 0 {
			procSelectObject.Call(dc, old)
		}
		cursorY += scaleDIP(28, ui.dpi)
	}
	if snapshot.Image != nil {
		imageHeight := ui.drawImage(dc, snapshot.Image, padding, cursorY, width-2*padding, height-cursorY-padding)
		cursorY += imageHeight + scaleDIP(12, ui.dpi)
	}
	if snapshot.Text != "" {
		font := ui.textFont
		old := uintptr(0)
		if font != 0 {
			old, _, _ = procSelectObject.Call(dc, font)
		}
		procSetTextColor.Call(dc, uintptr(rgb(55, 55, 55)))
		procSetBkMode.Call(dc, transparentBkMode)
		textPtr, _ := syscall.UTF16PtrFromString(snapshot.Text)
		textRect := uiRect{Left: padding, Top: cursorY, Right: width - padding, Bottom: height - padding}
		procDrawTextW.Call(dc, uintptr(unsafe.Pointer(textPtr)), uintptr(0xFFFFFFFF), uintptr(unsafe.Pointer(&textRect)), uintptr(dtLeft|dtTop|dtWordBreak|dtNoPrefix))
		runtime.KeepAlive(textPtr)
		if old != 0 {
			procSelectObject.Call(dc, old)
		}
	}
}

// 结构化视图布局参数（DIP，绘制时统一按 DPI 缩放）。
const (
	blockMarginX  = 12
	blockGapY     = 10
	cardPadX      = 14
	cardPadY      = 12
	cardGapInner  = 6
	badgeHeight   = 20
	badgePadX     = 8
	footerHeight  = 48
	footerMarginX = 12
	footerMarginY = 10
	scrollStepDIP = 48
	contentPadTop = 12
)

var blockToneColors = map[string][2]uint32{
	// [背景, 前景]
	"success": {rgb(220, 252, 231), rgb(22, 101, 52)},
	"danger":  {rgb(254, 226, 226), rgb(185, 28, 28)},
	"warning": {rgb(254, 243, 199), rgb(161, 94, 10)},
	"muted":   {rgb(243, 244, 246), rgb(107, 114, 128)},
}

// drawBlocks 渲染声明式 UI 块：浅灰页面底、白色圆角卡片、固定页眉与 footer，
// 内容区按 scrollY 裁剪滚动。所有布局在物理像素下计算。
func (ui *agentUIHandle) drawBlocks(dc uintptr, snapshot PanelSnapshot, width, height int32) {
	// 内容更新时重置滚动，避免新内容从中间开始。
	if !snapshot.UpdatedAt.Equal(ui.lastStamp) {
		ui.scrollY = 0
		ui.lastStamp = snapshot.UpdatedAt
	}

	brush, _, _ := procCreateSolidBrush.Call(uintptr(rgb(245, 245, 245)))
	rect := uiRect{Right: width, Bottom: height}
	procFillRect.Call(dc, uintptr(unsafe.Pointer(&rect)), brush)
	procDeleteObject.Call(brush)

	padding := scaleDIP(blockMarginX, ui.dpi)
	cursorY := scaleDIP(contentPadTop, ui.dpi)
	if snapshot.Title != "" {
		cursorY = ui.drawBlockText(dc, snapshot.Title, ui.titleFont, padding, cursorY, width-2*padding, rgb(30, 30, 30), dtWordBreak) + scaleDIP(10, ui.dpi)
	}

	footerH := int32(0)
	if snapshot.Footer != nil {
		footerH = scaleDIP(footerHeight+footerMarginY, ui.dpi)
	}
	contentTop := cursorY
	contentBottom := height - footerH

	// 计算内容总高并夹紧滚动偏移。
	totalH := int32(0)
	for index := range snapshot.Blocks {
		totalH += ui.blockHeight(&snapshot.Blocks[index]) + scaleDIP(blockGapY, ui.dpi)
	}
	viewportH := contentBottom - contentTop - scaleDIP(10, ui.dpi)
	maxScroll := totalH - viewportH
	if maxScroll < 0 {
		maxScroll = 0
	}
	if ui.scrollY > maxScroll {
		ui.scrollY = maxScroll
	}
	if ui.scrollY < 0 {
		ui.scrollY = 0
	}

	// 裁剪到内容区，卡片不越界到 footer。
	procSaveDC.Call(dc)
	clipRect := uiRect{Left: 0, Top: contentTop, Right: width, Bottom: contentBottom}
	procIntersectClipRect.Call(dc,
		uintptr(uint32(clipRect.Left)), uintptr(uint32(clipRect.Top)),
		uintptr(uint32(clipRect.Right)), uintptr(uint32(clipRect.Bottom)))

	y := contentTop - ui.scrollY
	for index := range snapshot.Blocks {
		block := &snapshot.Blocks[index]
		h := ui.drawBlock(dc, block, padding, y, width-2*padding)
		y += h + scaleDIP(blockGapY, ui.dpi)
	}
	_ = y
	procRestoreDC.Call(dc, 0xFFFFFFFF)

	// footer 固定在底部，记录命中矩形。
	ui.footerEnabled = false
	if snapshot.Footer != nil {
		left := scaleDIP(footerMarginX, ui.dpi)
		top := height - scaleDIP(footerHeight+footerMarginY, ui.dpi) + scaleDIP(footerMarginY/2, ui.dpi)
		btnRect := uiRect{Left: left, Top: top, Right: width - left, Bottom: top + scaleDIP(footerHeight, ui.dpi)}
		ui.footerRect = btnRect
		bg := rgb(37, 99, 235)
		if ui.footerPressed {
			bg = rgb(29, 78, 216)
		}
		ui.roundRectFill(dc, btnRect, scaleDIP(10, ui.dpi), bg)
		label := snapshot.Footer.Label
		labelW := ui.textWidth(dc, label, ui.footerFont)
		th := ui.textHeight(label, ui.footerFont)
		tx := btnRect.Left + (btnRect.Right-btnRect.Left-labelW)/2
		ty := btnRect.Top + (btnRect.Bottom-btnRect.Top-th)/2
		ui.drawTextAt(dc, label, ui.footerFont, tx, ty, rgb(255, 255, 255))
		if snapshot.Footer.Action != "" {
			ui.footerEnabled = true
		}
	} else {
		ui.footerRect = uiRect{}
	}
}

func (ui *agentUIHandle) drawBlock(dc uintptr, block *PanelBlock, x, y, w int32) int32 {
	if block.Kind == "stat" {
		return ui.drawStatBlock(dc, block, x, y, w)
	}
	return ui.drawCardBlock(dc, block, x, y, w)
}

func (ui *agentUIHandle) drawStatBlock(dc uintptr, block *PanelBlock, x, y, w int32) int32 {
	h := scaleDIP(72, ui.dpi)
	if block.Label != "" {
		// drawBlockText 返回结束 Y（绝对坐标），value 直接接在它后面。
		labelEnd := ui.drawBlockText(dc, block.Label, ui.smallFont, x, y+scaleDIP(8, ui.dpi), w, rgb(107, 114, 128), dtCenter)
		ui.drawBlockText(dc, block.Value, ui.statFont, x, labelEnd+scaleDIP(10, ui.dpi), w, rgb(17, 17, 27), dtCenter)
	} else {
		ui.drawBlockText(dc, block.Value, ui.statFont, x, y+scaleDIP(16, ui.dpi), w, rgb(17, 17, 27), dtCenter)
	}
	return h
}

func (ui *agentUIHandle) drawCardBlock(dc uintptr, block *PanelBlock, x, y, w int32) int32 {
	// 先量高：标题行 + 每行副文本 + 内边距。
	titleH := ui.textHeight(block.Title, ui.boldFont)
	h := scaleDIP(cardPadY, ui.dpi) + titleH + scaleDIP(cardPadY, ui.dpi)
	lineHeights := make([]int32, 0, len(block.Lines))
	for _, line := range block.Lines {
		lh := ui.textHeight(line, ui.smallFont)
		lineHeights = append(lineHeights, lh)
		h += scaleDIP(cardGapInner, ui.dpi) + lh
	}
	if h < scaleDIP(56, ui.dpi) {
		h = scaleDIP(56, ui.dpi)
	}

	cardRect := uiRect{Left: x, Top: y, Right: x + w, Bottom: y + h}
	ui.roundRectFill(dc, cardRect, scaleDIP(10, ui.dpi), rgb(255, 255, 255))

	cx := x + scaleDIP(cardPadX, ui.dpi)
	cy := y + scaleDIP(cardPadY, ui.dpi)
	titleRight := cardRect.Right - scaleDIP(cardPadX, ui.dpi)
	if block.Badge != nil {
		badgeW := ui.textWidth(dc, block.Badge.Text, ui.badgeFont) + 2*scaleDIP(badgePadX, ui.dpi)
		badgeH := scaleDIP(badgeHeight, ui.dpi)
		badgeRect := uiRect{
			Left:   cardRect.Right - scaleDIP(cardPadX, ui.dpi) - badgeW,
			Top:    cy,
			Right:  cardRect.Right - scaleDIP(cardPadX, ui.dpi),
			Bottom: cy + badgeH,
		}
		colors := blockToneColors[block.Badge.Tone]
		if colors == ([2]uint32{}) {
			colors = blockToneColors["muted"]
		}
		ui.roundRectFill(dc, badgeRect, badgeH/2, colors[0])
		labelW := ui.textWidth(dc, block.Badge.Text, ui.badgeFont)
		labelTH := ui.textHeight(block.Badge.Text, ui.badgeFont)
		ui.drawTextAt(dc, block.Badge.Text, ui.badgeFont,
			badgeRect.Left+(badgeW-labelW)/2,
			badgeRect.Top+(badgeH-labelTH)/2,
			colors[1])
		titleRight = badgeRect.Left - scaleDIP(cardGapInner, ui.dpi)
	}
	// 标题单行裁剪到徽章左侧。
	procSaveDC.Call(dc)
	titleClip := uiRect{Left: cx, Top: cy, Right: titleRight, Bottom: cy + titleH}
	procIntersectClipRect.Call(dc,
		uintptr(uint32(titleClip.Left)), uintptr(uint32(titleClip.Top)),
		uintptr(uint32(titleClip.Right)), uintptr(uint32(titleClip.Bottom)))
	ui.drawTextAt(dc, block.Title, ui.boldFont, cx, cy, rgb(17, 17, 27))
	procRestoreDC.Call(dc, 0xFFFFFFFF)

	cy += titleH
	for index, line := range block.Lines {
		cy += scaleDIP(cardGapInner, ui.dpi)
		ui.drawTextAt(dc, line, ui.smallFont, cx, cy, rgb(107, 114, 128))
		cy += lineHeights[index]
	}
	return h
}

func (ui *agentUIHandle) blockHeight(block *PanelBlock) int32 {
	if block.Kind == "stat" {
		return scaleDIP(72, ui.dpi)
	}
	h := scaleDIP(cardPadY, ui.dpi) + ui.textHeight(block.Title, ui.boldFont) + scaleDIP(cardPadY, ui.dpi)
	for _, line := range block.Lines {
		h += scaleDIP(cardGapInner, ui.dpi) + ui.textHeight(line, ui.smallFont)
	}
	if h < scaleDIP(56, ui.dpi) {
		h = scaleDIP(56, ui.dpi)
	}
	return h
}

// roundRectFill 用 RoundRect 画圆角矩形：笔刷和描边同色，填充后擦掉描边感。
func (ui *agentUIHandle) roundRectFill(dc uintptr, rect uiRect, radius int32, color uint32) {
	brush, _, _ := procCreateSolidBrush.Call(uintptr(color))
	pen, _, _ := procCreatePen.Call(psSolid, 1, uintptr(color))
	oldBrush, _, _ := procSelectObject.Call(dc, brush)
	oldPen, _, _ := procSelectObject.Call(dc, pen)
	procRoundRect.Call(dc,
		uintptr(uint32(rect.Left)), uintptr(uint32(rect.Top)),
		uintptr(uint32(rect.Right)), uintptr(uint32(rect.Bottom)),
		uintptr(uint32(radius)), uintptr(uint32(radius)))
	procSelectObject.Call(dc, oldBrush)
	procSelectObject.Call(dc, oldPen)
	procDeleteObject.Call(brush)
	procDeleteObject.Call(pen)
}

// drawBlockText 先用 DT_CALCRECT 量出文本高度，再在 (x, y) 绘制，返回结束 Y。
// 不依赖 DrawTextW 对 rect 的回写（Bottom=极大值时部分环境不更新，会把后续块画到标题上）。
func (ui *agentUIHandle) drawBlockText(dc uintptr, text string, font uintptr, x, y, w int32, color uint32, extraFlags uint32) int32 {
	if text == "" {
		return y
	}
	old := uintptr(0)
	if font != 0 {
		old, _, _ = procSelectObject.Call(dc, font)
	}
	procSetBkMode.Call(dc, transparentBkMode)
	textPtr, _ := syscall.UTF16PtrFromString(text)

	measureRect := uiRect{Left: x, Top: 0, Right: x + w, Bottom: 0}
	measureFlags := uint32(dtTop|dtNoPrefix|dtCalcrect) | extraFlags
	procDrawTextW.Call(dc, uintptr(unsafe.Pointer(textPtr)), uintptr(0xFFFFFFFF), uintptr(unsafe.Pointer(&measureRect)), uintptr(measureFlags))
	height := measureRect.Bottom - measureRect.Top
	if height <= 0 {
		height = scaleDIP(18, ui.dpi)
	}

	procSetTextColor.Call(dc, uintptr(color))
	drawRect := uiRect{Left: x, Top: y, Right: x + w, Bottom: y + height}
	drawFlags := uint32(dtLeft|dtTop|dtNoPrefix) | extraFlags
	procDrawTextW.Call(dc, uintptr(unsafe.Pointer(textPtr)), uintptr(0xFFFFFFFF), uintptr(unsafe.Pointer(&drawRect)), uintptr(drawFlags))
	runtime.KeepAlive(textPtr)
	if old != 0 {
		procSelectObject.Call(dc, old)
	}
	return y + height
}

func (ui *agentUIHandle) drawTextAt(dc uintptr, text string, font uintptr, x, y int32, color uint32) {
	if text == "" {
		return
	}
	old := uintptr(0)
	if font != 0 {
		old, _, _ = procSelectObject.Call(dc, font)
	}
	procSetTextColor.Call(dc, uintptr(color))
	procSetBkMode.Call(dc, transparentBkMode)
	textPtr, _ := syscall.UTF16PtrFromString(text)
	procTextOutW.Call(dc, uintptr(uint32(x)), uintptr(uint32(y)),
		uintptr(unsafe.Pointer(textPtr)), uintptr(len([]rune(text))))
	runtime.KeepAlive(textPtr)
	if old != 0 {
		procSelectObject.Call(dc, old)
	}
}

func (ui *agentUIHandle) textWidth(dc uintptr, text string, font uintptr) int32 {
	if text == "" || dc == 0 {
		return 0
	}
	old := uintptr(0)
	if font != 0 {
		old, _, _ = procSelectObject.Call(dc, font)
	}
	textPtr, _ := syscall.UTF16PtrFromString(text)
	var size uiPoint
	procGetTextExtentPoint32W.Call(dc,
		uintptr(unsafe.Pointer(textPtr)),
		uintptr(len([]rune(text))),
		uintptr(unsafe.Pointer(&size)))
	runtime.KeepAlive(textPtr)
	if old != 0 {
		procSelectObject.Call(dc, old)
	}
	return size.X
}

func (ui *agentUIHandle) textHeight(text string, font uintptr) int32 {
	// 逻辑近似：按字号估行高，不为量高创建临时 DC。
	line := scaleDIP(18, ui.dpi)
	if font == ui.titleFont {
		line = scaleDIP(28, ui.dpi)
	} else if font == ui.statFont {
		line = scaleDIP(38, ui.dpi)
	}
	if text == "" {
		return line
	}
	return line
}

// scrollBy 处理滚轮；返回是否消费了消息。
func (ui *agentUIHandle) scrollBy(wheelDelta int32) bool {
	snapshot := ui.panel.snapshot()
	if len(snapshot.Blocks) == 0 {
		return false
	}
	steps := wheelDelta / 120
	if steps == 0 {
		if wheelDelta > 0 {
			steps = 1
		} else if wheelDelta < 0 {
			steps = -1
		}
	}
	ui.mu.Lock()
	ui.scrollY -= steps * scaleDIP(scrollStepDIP, ui.dpi)
	if ui.scrollY < 0 {
		ui.scrollY = 0
	}
	ui.mu.Unlock()
	// 夹紧到内容底在 drawBlocks 里做；这里先请求重绘。
	procInvalidateRect.Call(ui.hwndOf(), 0, 1)
	return true
}

func (ui *agentUIHandle) hwndOf() uintptr {
	ui.mu.Lock()
	defer ui.mu.Unlock()
	return ui.hwnd
}

func (ui *agentUIHandle) beginFooterPress(x, y int32) bool {
	if !ui.footerEnabled || !pointInRect(x, y, ui.footerRect) {
		return false
	}
	ui.footerPressed = true
	procInvalidateRect.Call(ui.hwndOf(), uintptr(unsafe.Pointer(&ui.footerRect)), 1)
	return true
}

func (ui *agentUIHandle) completeFooterPress(x, y int32) bool {
	if !ui.footerPressed {
		return false
	}
	ui.footerPressed = false
	procInvalidateRect.Call(ui.hwndOf(), uintptr(unsafe.Pointer(&ui.footerRect)), 1)
	if pointInRect(x, y, ui.footerRect) {
		snapshot := ui.panel.snapshot()
		if snapshot.Footer != nil && snapshot.Footer.Action != "" {
			ui.panel.pushAction(snapshot.Footer.Action)
		}
	}
	return true
}

func pointInRect(x, y int32, rect uiRect) bool {
	return x >= rect.Left && x < rect.Right && y >= rect.Top && y < rect.Bottom
}

func (ui *agentUIHandle) drawAnimation(dc uintptr, width, height int32) {
	brush, _, _ := procCreateSolidBrush.Call(uintptr(rgb(0, 0, 0)))
	rect := uiRect{Right: width, Bottom: height}
	procFillRect.Call(dc, uintptr(unsafe.Pointer(&rect)), brush)
	procDeleteObject.Call(brush)

	anim := ui.ensureAnimation(width, height)
	ui.drawRain(dc, anim)
}

func (ui *agentUIHandle) drawRain(dc uintptr, anim *matrixAnimation) {
	if ui.rainFont != 0 {
		procSelectObject.Call(dc, ui.rainFont)
	}
	procSetBkMode.Call(dc, transparentBkMode)
	for index := range anim.columns {
		column := &anim.columns[index]
		for i := 0; i < column.length && i < len(column.glyphs); i++ {
			y := int32(column.y) - int32(i)*anim.lineHeight
			if y < -anim.lineHeight || y > anim.height {
				continue
			}
			if i == 0 {
				procSetTextColor.Call(dc, uintptr(rgb(220, 255, 220)))
			} else {
				fade := 1 - float64(i)/float64(column.length)
				green := byte(30 + 170*fade)
				blue := byte(20 + 50*fade)
				procSetTextColor.Call(dc, uintptr(rgb(0, green, blue)))
			}
			procTextOutW.Call(
				dc,
				uintptr(column.x),
				uintptr(y),
				uintptr(unsafe.Pointer(&column.glyphs[i])),
				1,
			)
		}
	}
}

func (ui *agentUIHandle) ensureAnimation(width, height int32) *matrixAnimation {
	if ui.anim != nil && ui.anim.width == width && ui.anim.height == height {
		return ui.anim
	}
	spacing := scaleDIP(13, ui.dpi)
	if spacing < 8 {
		spacing = 8
	}
	lineHeight := scaleDIP(15, ui.dpi)
	if lineHeight < 10 {
		lineHeight = 10
	}
	anim := &matrixAnimation{
		width:      width,
		height:     height,
		spacing:    spacing,
		lineHeight: lineHeight,
		glyphPool:  matrixGlyphPool(),
		rng:        rand.New(rand.NewSource(time.Now().UnixNano())),
	}
	columns := int(width/spacing) + 1
	for i := 0; i < columns; i++ {
		anim.columns = append(anim.columns, anim.newColumn(i))
	}
	ui.anim = anim
	return anim
}

func matrixGlyphPool() []uint16 {
	const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ@#$%&*+-=<>"
	pool := make([]uint16, 0, len(chars))
	for _, ch := range chars {
		pool = append(pool, uint16(ch))
	}
	return pool
}

func (a *matrixAnimation) newColumn(index int) rainColumn {
	length := 8 + a.rng.Intn(12)
	glyphs := make([]uint16, length)
	for i := range glyphs {
		glyphs[i] = a.glyphPool[a.rng.Intn(len(a.glyphPool))]
	}
	return rainColumn{
		x: int32(index) * a.spacing,
		// 初始位置覆盖整屏，打开面板立刻能看到代码雨，而不是等所有列从顶部落下。
		y:      float64(a.rng.Intn(int(a.height)*2) - int(a.height)),
		speed:  3.2 + a.rng.Float64()*5.2,
		length: length,
		glyphs: glyphs,
	}
}

func (a *matrixAnimation) advance() {
	a.phase += 0.08
	a.tick++
	for index := range a.columns {
		column := &a.columns[index]
		column.y += column.speed
		if column.y-float64(column.length)*float64(a.lineHeight) > float64(a.height) {
			*column = a.newColumn(index)
			column.y = -float64(a.rng.Intn(80) + 10)
		}
		for i := range column.glyphs {
			if a.rng.Intn(12) == 0 {
				column.glyphs[i] = a.glyphPool[a.rng.Intn(len(a.glyphPool))]
			}
		}
	}
}

func (ui *agentUIHandle) tickAnimation() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	width := ui.width
	height := ui.height
	ui.mu.Unlock()
	if hwnd == 0 || width <= 0 || height <= 0 {
		return
	}
	visible, _, _ := procIsWindowVisible.Call(hwnd)
	if visible == 0 {
		return
	}
	if iconic, _, _ := procIsIconic.Call(hwnd); iconic != 0 {
		return
	}
	snapshot := ui.panel.snapshot()
	if !panelSnapshotBlank(snapshot) {
		return
	}
	anim := ui.ensureAnimation(width, height)
	anim.advance()
	procInvalidateRect.Call(hwnd, 0, 0)
}

func (ui *agentUIHandle) drawImage(hdc uintptr, src *image.RGBA, left, top, maxWidth, maxHeight int32) int32 {
	if src == nil || maxWidth <= 0 || maxHeight <= 0 {
		return 0
	}
	bounds := src.Bounds()
	srcW := int32(bounds.Dx())
	srcH := int32(bounds.Dy())
	if srcW <= 0 || srcH <= 0 {
		return 0
	}
	destW := maxWidth
	destH := srcH * destW / srcW
	if destH > maxHeight {
		destH = maxHeight
		destW = srcW * destH / srcH
	}
	if destW <= 0 || destH <= 0 {
		return 0
	}
	destX := left + (maxWidth-destW)/2
	destY := top
	pixels, width, height := ui.imagePixelsFor(src)
	if len(pixels) == 0 {
		return 0
	}
	info := bitmapInfoHeader{
		Size:        uint32(unsafe.Sizeof(bitmapInfoHeader{})),
		Width:       int32(width),
		Height:      -int32(height),
		Planes:      1,
		BitCount:    32,
		Compression: 0,
		SizeImage:   uint32(len(pixels)),
	}
	procStretchDIBits.Call(
		hdc,
		uintptr(destX),
		uintptr(destY),
		uintptr(destW),
		uintptr(destH),
		0,
		0,
		uintptr(srcW),
		uintptr(srcH),
		uintptr(unsafe.Pointer(&pixels[0])),
		uintptr(unsafe.Pointer(&info)),
		dibRGBColors,
		srcCopy,
	)
	runtime.KeepAlive(pixels)
	return destH
}

func (ui *agentUIHandle) imagePixelsFor(src *image.RGBA) ([]byte, int, int) {
	ui.imageMu.Lock()
	defer ui.imageMu.Unlock()
	bounds := src.Bounds()
	width := bounds.Dx()
	height := bounds.Dy()
	if ui.imageRef == src && ui.imageWidth == width && ui.imageHeight == height {
		return ui.imagePixels, width, height
	}
	pixels := make([]byte, width*height*4)
	for y := 0; y < height; y++ {
		srcRow := src.PixOffset(bounds.Min.X, bounds.Min.Y+y)
		dstRow := y * width * 4
		for x := 0; x < width; x++ {
			offset := srcRow + x*4
			pixel := dstRow + x*4
			pixels[pixel] = src.Pix[offset+2]
			pixels[pixel+1] = src.Pix[offset+1]
			pixels[pixel+2] = src.Pix[offset]
			pixels[pixel+3] = 255
		}
	}
	ui.imageRef = src
	ui.imagePixels = pixels
	ui.imageWidth = width
	ui.imageHeight = height
	return pixels, width, height
}

func (ui *agentUIHandle) addTrayIcon() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	ui.mu.Unlock()
	if hwnd == 0 {
		return
	}
	data := ui.notifyData(hwnd)
	data.UFlags = nifMsg | nifIcon | nifTip
	copy(data.SzTip[:], utf16.Encode([]rune("Mode Go 正在运行")))
	ok, _, _ := procShellNotify.Call(nimAdd, uintptr(unsafe.Pointer(&data)))
	if ok != 0 {
		ui.tray = true
	}
}

func (ui *agentUIHandle) removeTrayIcon() {
	ui.mu.Lock()
	hwnd := ui.hwnd
	tray := ui.tray
	ui.mu.Unlock()
	if hwnd == 0 || !tray {
		return
	}
	data := ui.notifyData(hwnd)
	procShellNotify.Call(nimDelete, uintptr(unsafe.Pointer(&data)))
	ui.tray = false
}

func (ui *agentUIHandle) notifyData(hwnd uintptr) notifyIconDataW {
	return notifyIconDataW{
		CbSize:           uint32(unsafe.Sizeof(notifyIconDataW{})),
		HWnd:             hwnd,
		UID:              1,
		UCallbackMessage: wmTrayCallback,
		HIcon:            agentUIIcon,
	}
}

// installSystemMenu 把应用命令追加到 Windows 原生系统菜单：
// 标题栏图标点击时仍显示 还原/移动/大小/关闭 等原生项，并多了设置/配对/退出。
func installSystemMenu(hwnd uintptr) {
	menu, _, _ := procGetSystemMenu.Call(hwnd, 0)
	if menu == 0 {
		return
	}
	procAppendMenuW.Call(menu, mfSeparator, 0, 0)
	appendSystemMenuItem(menu, sysMenuSettings, "设置...")
	appendSystemMenuItem(menu, sysMenuPair, "登录 GitHub...")
	appendSystemMenuItem(menu, sysMenuExit, "退出 Mode Go")
}

func appendSystemMenuItem(menu, id uintptr, text string) {
	ptr, _ := syscall.UTF16PtrFromString(text)
	procAppendMenuW.Call(menu, mfString, id, uintptr(unsafe.Pointer(ptr)))
	runtime.KeepAlive(ptr)
}

func (ui *agentUIHandle) setTrayEnabled(enabled bool) {
	if enabled {
		ui.addTrayIcon()
		return
	}
	ui.removeTrayIcon()
}

func (ui *agentUIHandle) deleteFonts() {
	fonts := []*uintptr{&ui.titleFont, &ui.textFont, &ui.rainFont, &ui.boldFont, &ui.smallFont, &ui.statFont, &ui.badgeFont, &ui.footerFont}
	for _, slot := range fonts {
		if *slot != 0 {
			procDeleteObject.Call(*slot)
			*slot = 0
		}
	}
}

func syncAutostartForPanel(cfg *Config, explicitDisable bool) error {
	if cfg == nil || cfg.configPath == "" {
		return nil
	}
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	if cfg.Panel.Autostart {
		return installAutostartWithExe(exe, cfg.configPath)
	}
	// 只有用户明确把本实例的自启从开切到关时才删除 Run 项；
	// 否则会误删另一个已安装实例的 ModeAgent 项。
	if !explicitDisable {
		return nil
	}
	return uninstallAutostart()
}

func loadAgentIcon() uintptr {
	instance, _, _ := procGetModuleHandleW.Call(0)
	if icon, _, _ := procLoadIconW.Call(instance, 1); icon != 0 {
		return icon
	}
	icon, _, _ := procLoadIconW.Call(0, idiApplication)
	return icon
}

func systemDPI() int32 {
	dc, _, _ := procGetDC.Call(0)
	if dc == 0 {
		return 96
	}
	dpi, _, _ := procGetDeviceCaps.Call(dc, 88) // LOGPIXELSX
	procReleaseDC.Call(0, dc)
	if dpi >= 72 && dpi <= 480 {
		return int32(dpi)
	}
	return 96
}

func scaleDIP(value int32, dpi int32) int32 {
	if dpi <= 0 {
		dpi = 96
	}
	return value * dpi / 96
}

func centeredWindowPosition(width, height int32) (int32, int32) {
	var work uiRect
	ok, _, _ := procSystemParametersInfoW.Call(0x0030, 0, uintptr(unsafe.Pointer(&work)), 0) // SPI_GETWORKAREA
	if ok == 0 || work.Right <= work.Left || work.Bottom <= work.Top {
		return 100, 100
	}
	return work.Left + (work.Right-work.Left-width)/2, work.Top + (work.Bottom-work.Top-height)/2
}

func rgb(red, green, blue byte) uint32 {
	return uint32(red) | uint32(green)<<8 | uint32(blue)<<16
}

func hideConsoleForPanel() {
	procFreeConsole.Call()
}

func createUIFont(height int32, weight int32) uintptr {
	return createFontWithFace("Microsoft YaHei UI", height, weight)
}

func createFontWithFace(faceName string, height int32, weight int32) uintptr {
	face, _ := syscall.UTF16PtrFromString(faceName)
	font, _, _ := procCreateFontW.Call(
		uintptr(int32(-height)),
		0,
		0,
		0,
		uintptr(weight),
		0,
		0,
		0,
		defaultCharset,
		0,
		0,
		cleartypeQuality,
		0,
		uintptr(unsafe.Pointer(face)),
	)
	runtime.KeepAlive(face)
	return font
}
