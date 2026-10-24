//go:build darwin && cgo && !nogui

package gui

import (
	"fmt"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"
)

// Mitsuha on Discord (v0.1.1364) and audie: after an update the Mac's menu
// bar icon was gone, and quitting and reopening didn't bring it back. From
// v0.1.1354 a restart to update hands the gateway over, and the magpie
// before hid its item with setVisible:NO. AppKit keeps that in the app's
// defaults ("NSStatusItem VisibleCC Item-0" = 0), so the new version's item
// and every later one started hidden. The hide leaves nothing kept, and a
// hide an older magpie kept is undone once per computer; a later one is
// the user's and stays.
func TestHandoverLeavesTheMenuBarIconForTheNextMagpie(t *testing.T) {
	out, err := runAppKit(t, 30*time.Second, []string{"MAGPIE_TEST_TRAY_SHOWN=1"})
	if err != nil || !strings.Contains(string(out), "tray shown: ok") {
		t.Fatalf("tray shown: %v\n%s", err, out)
	}
}

func init() {
	if os.Getenv("MAGPIE_TEST_TRAY_SHOWN") != "1" {
		return
	}
	app := application.New(application.Options{
		Name: "magpie tray shown regression",
		Mac:  application.MacOptions{ActivationPolicy: application.ActivationPolicyAccessory},
		Assets: application.AssetOptions{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "text/html")
			fmt.Fprint(w, `<p>magpie</p>`)
		})},
	})
	h := &host{app: app}
	h.tray = app.SystemTray.New()
	h.tray.SetTemplateIcon(trayIcon)
	fail := func(format string, a ...any) {
		fmt.Printf("tray shown: "+format+"\n", a...)
		os.Exit(1)
	}
	app.Event.OnApplicationEvent(events.Common.ApplicationStarted, func(*application.ApplicationEvent) {
		go func() {
			// what a magpie before this fix did as it handed over: the hide
			// is kept for the app (the item is up a moment after the start)
			for deadline := time.Now().Add(5 * time.Second); trayHidden() == 0; {
				if time.Now().After(deadline) {
					fail("Hide kept nothing in the defaults; the cause isn't what this test is about")
				}
				h.tray.Hide()
				mainSettled()
				time.Sleep(50 * time.Millisecond)
			}
			if still := bringTrayBack(); still || trayHidden() != 0 {
				fail("the icon an older magpie hid wasn't brought back (still hidden %v, %d kept)", still, trayHidden())
			}
			h.tray.Show()
			mainSettled()

			// this magpie handing over
			h.leaveMenuBar()
			mainSettled()
			if n := trayHidden(); n != 0 {
				fail("handing over kept the menu bar icon hidden for the next magpie (%d kept)", n)
			}

			// the user's own hide, after the one-time bring-back (shown
			// again first: a hidden item's Hide keeps nothing)
			h.tray.Show()
			mainSettled()
			h.tray.Hide()
			mainSettled()
			if still := bringTrayBack(); !still || trayHidden() != 1 {
				fail("the user's own hide was undone (still hidden %v, %d kept)", still, trayHidden())
			}
			fmt.Println("tray shown: ok")
			os.Exit(0)
		}()
	})
	if err := app.Run(); err != nil {
		fail("run: %v", err)
	}
	os.Exit(1)
}
