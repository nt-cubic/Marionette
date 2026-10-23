package gui

import (
	"os"
	"path/filepath"
)

// webkitDefaults sets how WebKitGTK draws the pages before the app makes
// its first webview. Under Wayland its accelerated compositing flickers the
// window on a click or a hover (myxxts on Discord: Fedora 44, KDE, AMD
// Radeon 780M; Wails already turns DMA-BUF off for NVIDIA only), and with
// it off the pages draw steadily and, as they said, smoother. A user who
// set either variable keeps what they set; WEBKIT_DISABLE_COMPOSITING_MODE=0
// turns compositing back on.
//
// Under X11 with no GPU this process can draw with — an xrdp, VNC or Xvfb
// session, or a user outside the render group — the compositing falls back
// to Mesa's llvmpipe, which rasterises and blends every layer on the CPU
// with a thread per core: the Routing page took 459%–830% CPU there, and
// 23% with compositing off, looking the same (#2827, zhang-4411). So with
// no render node this process can open, compositing is off too.
func webkitDefaults() {
	webkitDefaultsFor(os.Getenv, os.Setenv, gpuReachable)
}

func webkitDefaultsFor(getenv func(string) string, setenv func(string, string) error, gpu func() bool) {
	if getenv("WEBKIT_DISABLE_COMPOSITING_MODE") != "" || getenv("WEBKIT_DISABLE_DMABUF_RENDERER") != "" {
		return
	}
	wayland := getenv("XDG_SESSION_TYPE") == "wayland" || getenv("WAYLAND_DISPLAY") != ""
	if !wayland && gpu() {
		return
	}
	setenv("WEBKIT_DISABLE_COMPOSITING_MODE", "1")
}

// gpuReachable reports whether this process can open a DRM render node,
// which is what EGL needs to draw on a GPU: none there, or none this user
// may open (libEGL's "failed to open /dev/dri/renderD128: Permission
// denied"), and WebKit's GL is software.
func gpuReachable() bool {
	nodes, _ := filepath.Glob("/dev/dri/renderD*")
	return renderNodeOpens(nodes, func(p string) error {
		f, err := os.OpenFile(p, os.O_RDWR, 0)
		if err == nil {
			f.Close()
		}
		return err
	})
}

func renderNodeOpens(nodes []string, open func(string) error) bool {
	for _, n := range nodes {
		if open(n) == nil {
			return true
		}
	}
	return false
}
