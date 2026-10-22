//go:build !nogui

package gui

import (
	"testing"

	"github.com/wailsapp/wails/v3/pkg/services/notifications"
)

// A Linux alert dismissed with its × opened the main window over the
// desktop with the focus (#2513): Wails reports the dismissal as
// DefaultActionIdentifier, so there only the click's own id opens Usage.
func TestDismissedAlertOpensNothing(t *testing.T) {
	alert, turn := "magpie-alert-1", turnNotice+"1"
	for _, c := range []struct {
		goos, id, action string
		want             bool
	}{
		{"linux", alert, notifications.DefaultActionIdentifier, false}, // dismissed
		{"linux", alert, linuxClick, true},                             // clicked
		{"linux", turn, linuxClick, false},
		{"darwin", alert, notifications.DefaultActionIdentifier, true},
		{"windows", alert, notifications.DefaultActionIdentifier, true},
		{"windows", turn, notifications.DefaultActionIdentifier, false},
	} {
		r := notifications.NotificationResponse{ID: c.id, ActionIdentifier: c.action}
		if got := opensUsage(c.goos, r); got != c.want {
			t.Errorf("opensUsage(%s, %s %s) = %v, want %v", c.goos, c.id, c.action, got, c.want)
		}
	}
}

// The click comes back as linuxClick only from a notification sent under
// clickCategory, whose action has that id.
func TestLinuxAlertSentUnderClickCategory(t *testing.T) {
	if o := notice("linux", "magpie-alert-1", "t", "b"); o.CategoryID != clickCategory.ID {
		t.Fatalf("Linux alert's category = %q, want %q", o.CategoryID, clickCategory.ID)
	}
	if a := clickCategory.Actions; len(a) != 1 || a[0].ID != linuxClick || linuxClick != "default" {
		t.Fatalf("clickCategory's actions = %+v, want one with the body's key %q", a, "default")
	}
	for _, goos := range []string{"darwin", "windows"} {
		if o := notice(goos, "magpie-alert-1", "t", "b"); o.CategoryID != "" {
			t.Errorf("%s alert's category = %q, want none", goos, o.CategoryID)
		}
	}
}
