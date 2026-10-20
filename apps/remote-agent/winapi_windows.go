// +build windows

package main

import (
	"os"
	"runtime"
	"sort"
	"strings"
	"sync"
	"syscall"
	"unicode/utf16"
	"unicode/utf8"
	"unsafe"
)

var (
	kernel32 = syscall.NewLazyDLL("kernel32.dll")
	user32   = syscall.NewLazyDLL("user32.dll")
	gdi32    = syscall.NewLazyDLL("gdi32.dll")
	ntdll    = syscall.NewLazyDLL("ntdll.dll")

	procCreateJobObjectW             = kernel32.NewProc("CreateJobObjectW")
	procAssignProcessToJobObject     = kernel32.NewProc("AssignProcessToJobObject")
	procTerminateJobObject           = kernel32.NewProc("TerminateJobObject")
	procOpenProcess                  = kernel32.NewProc("OpenProcess")
	procCloseHandle                  = kernel32.NewProc("CloseHandle")
	procTerminateProcess             = kernel32.NewProc("TerminateProcess")
	procGetCurrentProcessId          = kernel32.NewProc("GetCurrentProcessId")
	procCreateToolhelp32Snapshot     = kernel32.NewProc("CreateToolhelp32Snapshot")
	procProcess32FirstW              = kernel32.NewProc("Process32FirstW")
	procProcess32NextW               = kernel32.NewProc("Process32NextW")
	procGetTickCount                 = kernel32.NewProc("GetTickCount")
	procGlobalMemoryStatusEx         = kernel32.NewProc("GlobalMemoryStatusEx")
	procGetLogicalDriveStringsW      = kernel32.NewProc("GetLogicalDriveStringsW")
	procGetDriveTypeW                = kernel32.NewProc("GetDriveTypeW")
	procGetDiskFreeSpaceExW          = kernel32.NewProc("GetDiskFreeSpaceExW")
	procGetVersionExW                = kernel32.NewProc("GetVersionExW")
	procProcessIdToSessionId         = kernel32.NewProc("ProcessIdToSessionId")
	procWTSGetActiveConsoleSessionID = kernel32.NewProc("WTSGetActiveConsoleSessionId")
	procMultiByteToWideChar          = kernel32.NewProc("MultiByteToWideChar")
	procMoveFileExW                  = kernel32.NewProc("MoveFileExW")
	procRtlGetVersion                = ntdll.NewProc("RtlGetVersion")

	procGetDC                = user32.NewProc("GetDC")
	procReleaseDC            = user32.NewProc("ReleaseDC")
	procGetSystemMetrics     = user32.NewProc("GetSystemMetrics")
	procSetProcessDPIAware   = user32.NewProc("SetProcessDPIAware")
	procSendInput            = user32.NewProc("SendInput")
	procEnumWindows          = user32.NewProc("EnumWindows")
	procIsWindowVisible      = user32.NewProc("IsWindowVisible")
	procGetWindowTextLengthW = user32.NewProc("GetWindowTextLengthW")
	procGetWindowTextW       = user32.NewProc("GetWindowTextW")
	procGetWindowThreadPID   = user32.NewProc("GetWindowThreadProcessId")
	procGetWindowRect        = user32.NewProc("GetWindowRect")
	procIsWindow             = user32.NewProc("IsWindow")
	procIsIconic             = user32.NewProc("IsIconic")
	procShowWindow           = user32.NewProc("ShowWindow")
	procSetForegroundWindow  = user32.NewProc("SetForegroundWindow")
	procVkKeyScanW           = user32.NewProc("VkKeyScanW")
	procMessageBoxW          = user32.NewProc("MessageBoxW")

	procCreateCompatibleDC = gdi32.NewProc("CreateCompatibleDC")
	procCreateDIBSection   = gdi32.NewProc("CreateDIBSection")
	procSelectObject       = gdi32.NewProc("SelectObject")
	procBitBlt             = gdi32.NewProc("BitBlt")
	procDeleteObject       = gdi32.NewProc("DeleteObject")
	procDeleteDC           = gdi32.NewProc("DeleteDC")
)

const (
	smCXScreen        = 0
	smCYScreen        = 1
	smXVirtualScreen  = 76
	smYVirtualScreen  = 77
	smCXVirtualScreen = 78
	smCYVirtualScreen = 79

	srcCopy      = 0x00CC0020
	captureBlt   = 0x40000000
	dibRGBColors = 0

	processTerminate  = 0x0001
	processSetQuota   = 0x0100
	th32csSnapProcess = 0x00000002
	invalidHandle     = ^uintptr(0)

	swRestore = 9

	keyEventKeyUp   = 0x0002
	keyEventUnicode = 0x0004

	moveFileReplaceExisting = 0x00000001
	moveFileWriteThrough    = 0x00000008

	mbOK              = 0x00000000
	mbOKCancel        = 0x00000001
	mbYesNo           = 0x00000004
	mbIconQuestion    = 0x00000020
	mbIconInformation = 0x00000040
	mbIconWarning     = 0x00000030
	mbYes             = 6
	mbNo              = 7
)

func messageBox(title, text string, flags uint32) int {
	titlePtr, _ := syscall.UTF16PtrFromString(title)
	textPtr, _ := syscall.UTF16PtrFromString(text)
	result, _, _ := procMessageBoxW.Call(
		0,
		uintptr(unsafe.Pointer(textPtr)),
		uintptr(unsafe.Pointer(titlePtr)),
		uintptr(flags),
	)
	runtime.KeepAlive(titlePtr)
	runtime.KeepAlive(textPtr)
	return int(result)
}

type osVersionInfoW struct {
	Size       uint32
	Major      uint32
	Minor      uint32
	Build      uint32
	PlatformID uint32
	CSDVersion [128]uint16
}

type memoryStatusEx struct {
	Length               uint32
	MemoryLoad           uint32
	TotalPhys            uint64
	AvailPhys            uint64
	TotalPageFile        uint64
	AvailPageFile        uint64
	TotalVirtual         uint64
	AvailVirtual         uint64
	AvailExtendedVirtual uint64
}

type processEntry32 struct {
	Size            uint32
	Usage           uint32
	ProcessID       uint32
	DefaultHeapID   uintptr
	ModuleID        uint32
	Threads         uint32
	ParentProcessID uint32
	PriClassBase    int32
	Flags           uint32
	ExeFile         [260]uint16
}

type rect struct {
	Left   int32
	Top    int32
	Right  int32
	Bottom int32
}

type bitmapInfoHeader struct {
	Size          uint32
	Width         int32
	Height        int32
	Planes        uint16
	BitCount      uint16
	Compression   uint32
	SizeImage     uint32
	XPelsPerMeter int32
	YPelsPerMeter int32
	ClrUsed       uint32
	ClrImportant  uint32
}

var dpiOnce sync.Once

func setDPIAware() {
	dpiOnce.Do(func() {
		if procSetProcessDPIAware.Find() == nil {
			procSetProcessDPIAware.Call()
		}
	})
}

func platformInfo() (*InfoResult, *APIError) {
	setDPIAware()
	hostname, _ := os.Hostname()
	user := strings.TrimSpace(os.Getenv("USERNAME"))
	if domain := strings.TrimSpace(os.Getenv("USERDOMAIN")); domain != "" && user != "" {
		user = domain + `\` + user
	}
	versionName, build := windowsVersion()
	width, height := screenMetrics()
	sessionID, activeSessionID := sessionInfo()
	totalMemory, availableMemory := memoryStatus()
	return &InfoResult{
		Hostname:             hostname,
		User:                 user,
		OS:                   runtime.GOOS,
		Arch:                 runtime.GOARCH,
		WindowsVersion:       versionName,
		WindowsBuild:         build,
		SessionID:            sessionID,
		ActiveSessionID:      activeSessionID,
		DesktopAvailable:     sessionID == activeSessionID,
		UptimeSeconds:        int64(procGetTickCountValue() / 1000),
		MemoryTotalBytes:     totalMemory,
		MemoryAvailableBytes: availableMemory,
		Screen:               ScreenSize{Width: width, Height: height},
		Drives:               listDrives(),
	}, nil
}

func procGetTickCountValue() uint32 {
	value, _, _ := procGetTickCount.Call()
	return uint32(value)
}

func readWindowsVersionInfo() osVersionInfoW {
	info := osVersionInfoW{Size: uint32(unsafe.Sizeof(osVersionInfoW{}))}
	read := false
	if procRtlGetVersion.Find() == nil {
		status, _, _ := procRtlGetVersion.Call(uintptr(unsafe.Pointer(&info)))
		read = status == 0
	}
	if !read {
		info = osVersionInfoW{Size: uint32(unsafe.Sizeof(osVersionInfoW{}))}
		procGetVersionExW.Call(uintptr(unsafe.Pointer(&info)))
	}
	return info
}

func windowsVersionNumbers() (uint32, uint32, uint32) {
	info := readWindowsVersionInfo()
	return info.Major, info.Minor, info.Build
}

func windowsVersion() (string, uint32) {
	info := readWindowsVersionInfo()
	name := windowsVersionName(info.Major, info.Minor, info.Build)
	if csd := strings.TrimSpace(syscall.UTF16ToString(info.CSDVersion[:])); csd != "" {
		name += " " + csd
	}
	return name, info.Build
}

func windowsVersionName(major, minor, build uint32) string {
	switch {
	case major == 5 && minor == 0:
		return "Windows 2000"
	case major == 5 && minor == 1:
		return "Windows XP"
	case major == 5 && minor == 2:
		return "Windows XP x64 / Server 2003"
	case major == 6 && minor == 0:
		return "Windows Vista"
	case major == 6 && minor == 1:
		return "Windows 7"
	case major == 6 && minor == 2:
		return "Windows 8"
	case major == 6 && minor == 3:
		return "Windows 8.1"
	case major == 10 && build >= 22000:
		return "Windows 11"
	case major == 10:
		return "Windows 10"
	default:
		return "Windows"
	}
}

func screenMetrics() (int, int) {
	width := int32(procGetSystemMetricsValue(smCXVirtualScreen))
	height := int32(procGetSystemMetricsValue(smCYVirtualScreen))
	if width <= 0 || height <= 0 {
		width = int32(procGetSystemMetricsValue(smCXScreen))
		height = int32(procGetSystemMetricsValue(smCYScreen))
	}
	if width <= 0 || height <= 0 {
		width, height = 1024, 768
	}
	return int(width), int(height)
}

func virtualScreenOrigin() (int, int, int, int) {
	left := int(int32(procGetSystemMetricsValue(smXVirtualScreen)))
	top := int(int32(procGetSystemMetricsValue(smYVirtualScreen)))
	width := int(int32(procGetSystemMetricsValue(smCXVirtualScreen)))
	height := int(int32(procGetSystemMetricsValue(smCYVirtualScreen)))
	if width <= 0 || height <= 0 {
		left, top = 0, 0
		width = int(int32(procGetSystemMetricsValue(smCXScreen)))
		height = int(int32(procGetSystemMetricsValue(smCYScreen)))
	}
	if width <= 0 || height <= 0 {
		width, height = 1024, 768
	}
	return left, top, width, height
}

func procGetSystemMetricsValue(index int) int {
	value, _, _ := procGetSystemMetrics.Call(uintptr(index))
	return int(int32(value))
}

func sessionInfo() (uint32, uint32) {
	var session uint32
	pid, _, _ := procGetCurrentProcessId.Call()
	procProcessIdToSessionId.Call(pid, uintptr(unsafe.Pointer(&session)))
	// XP 的控制台会话是 0；WTS API 缺失时按当前会话处理，避免误报 desktopAvailable=false。
	active := session
	if procWTSGetActiveConsoleSessionID.Find() == nil {
		value, _, _ := procWTSGetActiveConsoleSessionID.Call()
		active = uint32(value)
	}
	return session, active
}

func memoryStatus() (uint64, uint64) {
	status := memoryStatusEx{Length: uint32(unsafe.Sizeof(memoryStatusEx{}))}
	ok, _, _ := procGlobalMemoryStatusEx.Call(uintptr(unsafe.Pointer(&status)))
	if ok == 0 {
		return 0, 0
	}
	return status.TotalPhys, status.AvailPhys
}

func listDrives() []DriveInfo {
	bufferSize, _, _ := procGetLogicalDriveStringsW.Call(0, 0)
	if bufferSize == 0 {
		return nil
	}
	buffer := make([]uint16, int(bufferSize)+1)
	written, _, _ := procGetLogicalDriveStringsW.Call(uintptr(len(buffer)), uintptr(unsafe.Pointer(&buffer[0])))
	runtime.KeepAlive(buffer)
	if written == 0 {
		return nil
	}
	var drives []DriveInfo
	for index := 0; index < len(buffer); {
		start := index
		for index < len(buffer) && buffer[index] != 0 {
			index++
		}
		if index == start {
			break
		}
		path := syscall.UTF16ToString(buffer[start:index])
		index++
		if path == "" {
			continue
		}
		driveType := driveTypeName(path)
		var freeAvailable, totalBytes, totalFree uint64
		pathPtr, _ := syscall.UTF16PtrFromString(path)
		ok, _, _ := procGetDiskFreeSpaceExW.Call(
			uintptr(unsafe.Pointer(pathPtr)),
			uintptr(unsafe.Pointer(&freeAvailable)),
			uintptr(unsafe.Pointer(&totalBytes)),
			uintptr(unsafe.Pointer(&totalFree)),
		)
		if ok == 0 {
			continue
		}
		drives = append(drives, DriveInfo{
			Path:       path,
			Type:       driveType,
			TotalBytes: totalBytes,
			FreeBytes:  freeAvailable,
		})
	}
	sort.Slice(drives, func(i, j int) bool { return drives[i].Path < drives[j].Path })
	return drives
}

func driveTypeName(path string) string {
	pathPtr, _ := syscall.UTF16PtrFromString(path)
	value, _, _ := procGetDriveTypeW.Call(uintptr(unsafe.Pointer(pathPtr)))
	switch uint32(value) {
	case 2:
		return "removable"
	case 3:
		return "fixed"
	case 4:
		return "network"
	case 5:
		return "cdrom"
	case 6:
		return "ramdisk"
	default:
		return "unknown"
	}
}

func processList() (*ProcessListResult, *APIError) {
	snapshot, _, err := procCreateToolhelp32Snapshot.Call(th32csSnapProcess, 0)
	if snapshot == invalidHandle {
		return nil, apiError("process_list_failed", "CreateToolhelp32Snapshot failed: "+err.Error(), 500)
	}
	defer procCloseHandle.Call(snapshot)
	entry := processEntry32{Size: uint32(unsafe.Sizeof(processEntry32{}))}
	ok, _, err := procProcess32FirstW.Call(snapshot, uintptr(unsafe.Pointer(&entry)))
	if ok == 0 {
		return nil, apiError("process_list_failed", "Process32First failed: "+err.Error(), 500)
	}
	result := &ProcessListResult{Processes: make([]ProcessInfo, 0, 128)}
	for {
		result.Processes = append(result.Processes, ProcessInfo{
			PID:       entry.ProcessID,
			Name:      syscall.UTF16ToString(entry.ExeFile[:]),
			Threads:   entry.Threads,
			ParentPID: entry.ParentProcessID,
		})
		entry.Size = uint32(unsafe.Sizeof(processEntry32{}))
		ok, _, _ = procProcess32NextW.Call(snapshot, uintptr(unsafe.Pointer(&entry)))
		if ok == 0 {
			break
		}
	}
	sort.Slice(result.Processes, func(i, j int) bool { return result.Processes[i].PID < result.Processes[j].PID })
	return result, nil
}

func processKill(req ProcessKillRequest) (*ProcessKillResult, *APIError) {
	if req.PID == 0 || req.PID == 4 {
		return nil, apiError("process_kill_failed", "refusing to terminate a system process", 400)
	}
	currentPID, _, _ := procGetCurrentProcessId.Call()
	if uint32(currentPID) == req.PID {
		return nil, apiError("process_kill_failed", "refusing to terminate the agent itself", 400)
	}
	handle, _, err := procOpenProcess.Call(processTerminate, 0, uintptr(req.PID))
	if handle == 0 {
		return nil, apiError("process_kill_failed", "OpenProcess failed: "+err.Error(), 404)
	}
	defer procCloseHandle.Call(handle)
	ok, _, err := procTerminateProcess.Call(handle, 1)
	if ok == 0 {
		return nil, apiError("process_kill_failed", "TerminateProcess failed: "+err.Error(), 500)
	}
	return &ProcessKillResult{PID: req.PID, Killed: true}, nil
}

func newJobObject() uintptr {
	handle, _, _ := procCreateJobObjectW.Call(0, 0)
	return handle
}

func assignJobObject(jobObject uintptr, pid int) bool {
	if jobObject == 0 {
		return false
	}
	process, _, _ := procOpenProcess.Call(processTerminate|processSetQuota, 0, uintptr(pid))
	if process == 0 {
		return false
	}
	defer procCloseHandle.Call(process)
	ok, _, _ := procAssignProcessToJobObject.Call(jobObject, process)
	return ok != 0
}

func terminateJobObject(jobObject uintptr) {
	if jobObject != 0 {
		procTerminateJobObject.Call(jobObject, 1)
	}
}

func closeJobObject(jobObject uintptr) {
	if jobObject != 0 {
		procCloseHandle.Call(jobObject)
	}
}

func decodeConsoleBytes(data []byte, mode string) string {
	if len(data) == 0 {
		return ""
	}
	mode = strings.ToLower(strings.TrimSpace(mode))
	if mode == "utf8" || mode == "utf-8" {
		return sanitizeUTF8(data)
	}
	if mode == "" || mode == "auto" {
		if utf8.Valid(data) {
			return string(data)
		}
		return decodeCodePage(data, 1) // CP_OEMCP
	}
	codePage := uint32(0) // CP_ACP
	if mode == "oem" {
		codePage = 1
	}
	return decodeCodePage(data, codePage)
}

func decodeCodePage(data []byte, codePage uint32) string {
	if len(data) == 0 {
		return ""
	}
	count, _, _ := procMultiByteToWideChar.Call(
		uintptr(codePage), 0,
		uintptr(unsafe.Pointer(&data[0])), uintptr(len(data)),
		0, 0,
	)
	if count == 0 {
		return sanitizeUTF8(data)
	}
	buffer := make([]uint16, int(count))
	written, _, _ := procMultiByteToWideChar.Call(
		uintptr(codePage), 0,
		uintptr(unsafe.Pointer(&data[0])), uintptr(len(data)),
		uintptr(unsafe.Pointer(&buffer[0])), uintptr(len(buffer)),
	)
	runtime.KeepAlive(data)
	runtime.KeepAlive(buffer)
	if written == 0 {
		return sanitizeUTF8(data)
	}
	return string(utf16.Decode(buffer[:int(written)]))
}

func replaceFile(source, target string) error {
	sourcePtr, err := syscall.UTF16PtrFromString(source)
	if err != nil {
		return err
	}
	targetPtr, err := syscall.UTF16PtrFromString(target)
	if err != nil {
		return err
	}
	ok, _, callErr := procMoveFileExW.Call(
		uintptr(unsafe.Pointer(sourcePtr)),
		uintptr(unsafe.Pointer(targetPtr)),
		moveFileReplaceExisting|moveFileWriteThrough,
	)
	runtime.KeepAlive(sourcePtr)
	runtime.KeepAlive(targetPtr)
	if ok == 0 {
		return callErr
	}
	return nil
}

func putUint16(buffer []byte, offset int, value uint16) {
	buffer[offset] = byte(value)
	buffer[offset+1] = byte(value >> 8)
}

func putUint32(buffer []byte, offset int, value uint32) {
	buffer[offset] = byte(value)
	buffer[offset+1] = byte(value >> 8)
	buffer[offset+2] = byte(value >> 16)
	buffer[offset+3] = byte(value >> 24)
}

func putUintptr(buffer []byte, offset int, value uintptr) {
	putUint32(buffer, offset, uint32(value))
	if unsafe.Sizeof(uintptr(0)) == 8 {
		putUint32(buffer, offset+4, uint32(uint64(value)>>32))
	}
}
