package gui

import (
	"errors"
	"testing"
)

// Under Wayland the pages are drawn without accelerated compositing, which
// flickered (myxxts); under X11 too when no GPU can be reached, where
// compositing fell back to llvmpipe and took 459%–830% CPU (#2827). X11
// with a GPU is left alone, and so is a user's own setting.
func TestWebkitDefaults(t *testing.T) {
	for _, c := range []struct {
		name string
		env  map[string]string
		gpu  bool
		want string
	}{
		{"wayland", map[string]string{"XDG_SESSION_TYPE": "wayland"}, true, "1"},
		{"wayland display only", map[string]string{"WAYLAND_DISPLAY": "wayland-0"}, true, "1"},
		{"x11", map[string]string{"XDG_SESSION_TYPE": "x11"}, true, ""},
		{"x11 without a gpu (xrdp)", map[string]string{"XDG_SESSION_TYPE": "x11", "DISPLAY": ":10"}, false, "1"},
		{"user turned it on again", map[string]string{"XDG_SESSION_TYPE": "wayland", "WEBKIT_DISABLE_COMPOSITING_MODE": "0"}, true, "0"},
		{"user turned it on again without a gpu", map[string]string{"XDG_SESSION_TYPE": "x11", "WEBKIT_DISABLE_COMPOSITING_MODE": "0"}, false, "0"},
		{"user chose dmabuf off", map[string]string{"XDG_SESSION_TYPE": "wayland", "WEBKIT_DISABLE_DMABUF_RENDERER": "1"}, true, ""},
		{"user chose dmabuf off without a gpu", map[string]string{"XDG_SESSION_TYPE": "x11", "WEBKIT_DISABLE_DMABUF_RENDERER": "1"}, false, ""},
	} {
		env := map[string]string{}
		for k, v := range c.env {
			env[k] = v
		}
		webkitDefaultsFor(func(k string) string { return env[k] }, func(k, v string) error { env[k] = v; return nil }, func() bool { return c.gpu })
		if got := env["WEBKIT_DISABLE_COMPOSITING_MODE"]; got != c.want {
			t.Errorf("%s: WEBKIT_DISABLE_COMPOSITING_MODE=%q, want %q", c.name, got, c.want)
		}
	}
}

// A GPU is reachable when one render node opens: none there, or each one
// refused (the xrdp user outside the render group), is software drawing.
func TestRenderNodeOpens(t *testing.T) {
	denied := errors.New("permission denied")
	for _, c := range []struct {
		name  string
		nodes []string
		ok    map[string]bool
		want  bool
	}{
		{"no render node", nil, nil, false},
		{"every node refused", []string{"/dev/dri/renderD128", "/dev/dri/renderD129"}, nil, false},
		{"the second node opens", []string{"/dev/dri/renderD128", "/dev/dri/renderD129"}, map[string]bool{"/dev/dri/renderD129": true}, true},
	} {
		got := renderNodeOpens(c.nodes, func(p string) error {
			if c.ok[p] {
				return nil
			}
			return denied
		})
		if got != c.want {
			t.Errorf("%s: renderNodeOpens = %v, want %v", c.name, got, c.want)
		}
	}
}
