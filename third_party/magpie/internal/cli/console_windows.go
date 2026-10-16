package cli

import (
	"os"
	"syscall"
)

// The desktop build is linked as a GUI program so double-clicking it opens
// no console window; such a program starts with no standard streams. When it
// is run from a terminal with a command, borrow that terminal's console so
// `magpie ls` and friends still print. Output that is redirected already has
// somewhere to go and is left alone.
func init() {
	if len(os.Args) < 2 {
		return
	}
	if t, err := syscall.GetFileType(syscall.Stdout); err == nil && t != 0 {
		return // FILE_TYPE_UNKNOWN is 0: nothing attached
	}
	const attachParentProcess = ^uintptr(0) // (DWORD)-1
	r, _, _ := syscall.NewLazyDLL("kernel32.dll").NewProc("AttachConsole").Call(attachParentProcess)
	if r == 0 {
		return
	}
	if out, err := os.OpenFile("CONOUT$", os.O_RDWR, 0); err == nil {
		os.Stdout, os.Stderr = out, out
	}
	if in, err := os.OpenFile("CONIN$", os.O_RDWR, 0); err == nil {
		os.Stdin = in
	}
}
