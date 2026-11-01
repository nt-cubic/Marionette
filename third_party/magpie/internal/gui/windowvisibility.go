//go:build !nogui

package gui

import (
	"runtime"

	"github.com/wailsapp/wails/v3/pkg/application"
)

// A macOS window hidden with orderOut keeps its WKWebView's page visible, so
// tell the page before hiding it and tell it again when it is shown. Only
// the Mac is told: WebView2 and WebKitGTK mark a hidden window's page hidden
// themselves, and there the panel is also shown by Wails' ToggleWindow,
// which would never tell the page it is back (#4293).
const (
	pageHiddenJS = "window.magpieWindowVisibility?.(false)"
	pageShownJS  = "window.magpieWindowVisibility?.(true)"
)

func hideWebview(w *application.WebviewWindow) {
	if w == nil {
		return
	}
	if js := pageHiddenJSOn(runtime.GOOS); js != "" {
		w.ExecJS(js)
	}
	w.Hide()
}

func showWebview(w *application.WebviewWindow) *application.WebviewWindow {
	if w == nil {
		return nil
	}
	w.Show()
	w.ExecJS(pageShownJS)
	return w
}

// pageHiddenJSOn is what a page on goos is told as its window hides.
func pageHiddenJSOn(goos string) string {
	if goos != "darwin" {
		return ""
	}
	return pageHiddenJS
}
