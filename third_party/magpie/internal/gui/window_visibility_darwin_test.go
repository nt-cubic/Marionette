//go:build darwin && cgo && !nogui

package gui

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"
)

// A closed NSWindow keeps its WKWebView's document visible unless magpie
// says otherwise. The page here records what it saw as it was hidden, and
// reports that record once it is shown again; without hideWebview's signal
// it remains a visible page with running animations.
func TestClosingWindowMarksPageOutOfSight(t *testing.T) {
	out, err := runAppKit(t, 30*time.Second, []string{"MAGPIE_TEST_WINDOW_VISIBILITY=1"})
	if err != nil || !strings.Contains(string(out), "window visibility: ok") {
		t.Fatalf("window visibility: %v\n%s", err, out)
	}
}

func init() {
	if os.Getenv("MAGPIE_TEST_WINDOW_VISIBILITY") != "1" {
		return
	}
	ready := make(chan struct{}, 1)
	reports := make(chan url.Values, 1)
	app := application.New(application.Options{
		Name: "magpie window visibility regression",
		Mac:  application.MacOptions{ActivationPolicy: application.ActivationPolicyProhibited},
		Assets: application.AssetOptions{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			switch r.URL.Path {
			case "/":
				w.Header().Set("Content-Type", "text/html")
				fmt.Fprint(w, `<!doctype html><script type="module">import "/wails/runtime.js";</script><style>
#probe{animation:spin 1s linear infinite}@keyframes spin{to{transform:rotate(1turn)}}
html.magpie-out-of-sight #probe{animation-play-state:paused!important}
</style><i id="probe"></i><script>
const probe = document.querySelector("#probe");
window.magpieWindowVisibility = (visible) => {
  document.documentElement.classList.toggle("magpie-out-of-sight", !visible);
  document.dispatchEvent(new Event("visibilitychange"));
};
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    localStorage.setItem("hiddenProbe", JSON.stringify({
      hidden: document.hidden,
      state: document.visibilityState,
      marked: document.documentElement.classList.contains("magpie-out-of-sight"),
      play: getComputedStyle(probe).animationPlayState,
    }));
  }
});
setInterval(() => {
  const probe = localStorage.getItem("hiddenProbe");
  if (probe) fetch("/report?probe=" + encodeURIComponent(probe));
}, 100);
fetch("/ready");
</script>`)
			case "/ready":
				select {
				case ready <- struct{}{}:
				default:
				}
				w.WriteHeader(http.StatusNoContent)
			case "/report":
				select {
				case reports <- r.URL.Query():
				default:
				}
				w.WriteHeader(http.StatusNoContent)
			default:
				http.NotFound(w, r)
			}
		})},
	})
	h := &host{app: app}
	fail := func(format string, a ...any) {
		fmt.Printf("window visibility: "+format+"\n", a...)
		os.Exit(1)
	}
	wait := func(what string, ch <-chan url.Values) url.Values {
		select {
		case v := <-ch:
			return v
		case <-time.After(8 * time.Second):
			fail("timed out waiting for %s", what)
			return nil
		}
	}
	app.Event.OnApplicationEvent(events.Common.ApplicationStarted, func(*application.ApplicationEvent) {
		go func() {
			var win *application.WebviewWindow
			application.InvokeSync(func() {
				win = app.Window.NewWithOptions(application.WebviewWindowOptions{
					Name:   "visibility",
					Title:  "visibility",
					URL:    "/",
					Width:  480,
					Height: 320,
				})
				h.main = win
			})
			select {
			case <-ready:
			case <-time.After(8 * time.Second):
				fail("timed out waiting for the page")
			}

			application.InvokeSync(h.hideMain)
			time.Sleep(500 * time.Millisecond)
			application.InvokeSync(func() { showWebview(win).Focus() })
			report := wait("the page's hidden report", reports)
			probe := struct {
				Hidden bool   `json:"hidden"`
				State  string `json:"state"`
				Marked bool   `json:"marked"`
				Play   string `json:"play"`
			}{}
			if err := json.Unmarshal([]byte(report.Get("probe")), &probe); err != nil {
				fail("the page's hidden event is %q: %v", report.Get("probe"), err)
			}
			if !probe.Hidden || probe.State != "hidden" || !probe.Marked || probe.Play != "paused" {
				fail("while hidden the page saw hidden=%v state=%q marked=%v play=%q", probe.Hidden, probe.State, probe.Marked, probe.Play)
			}
			fmt.Println("window visibility: ok")
			app.Quit()
		}()
	})
	if err := app.Run(); err != nil {
		panic(err)
	}
	os.Exit(0)
}
