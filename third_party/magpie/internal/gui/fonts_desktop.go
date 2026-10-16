//go:build !nogui

package gui

import (
	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/yetone/magpie/internal/settings"
)

func (h *host) fontsChanged() {
	application.InvokeSync(func() {
		// Read the latest saved choice here, so simultaneous saves cannot
		// deliver an older notification after a newer one. A released
		// lightweight webview will get its choices from boot.js instead.
		js := fontSettingsJS(settings.Load())
		for _, w := range []*application.WebviewWindow{h.main, h.panel} {
			if w != nil {
				w.ExecJS(js)
			}
		}
	})
}
