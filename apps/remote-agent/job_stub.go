// +build !windows

package main

func newJobObject() uintptr {
	return 0
}

func assignJobObject(jobObject uintptr, pid int) bool {
	return false
}

func terminateJobObject(jobObject uintptr) {
}

func closeJobObject(jobObject uintptr) {
}
