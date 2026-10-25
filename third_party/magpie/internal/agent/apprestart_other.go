//go:build !windows

package agent

import "errors"

var errNotWindows = errors.New("only on Windows")

func listWinProcs(string) []winProc { return nil }
func closeWinProcs([]int) error     { return errNotWindows }
func endWinTree(int) error          { return errNotWindows }
func launchWin(winProc) error       { return errNotWindows }
