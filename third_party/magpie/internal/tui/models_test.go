package tui

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/provider"
)

// fakeCursor signs a Cursor account in through a cursor-agent of the test's
// own, the way the real CLI answers `about` and `models`. While down is
// there, `models` fails, as a CLI with no network does.
func fakeCursor(t *testing.T, h string) (down string) {
	t.Helper()
	cli := filepath.Join(h, "cursor-agent")
	down = filepath.Join(h, "cursor-down")
	write(t, cli, `#!/bin/sh
case "$1" in
about) echo '{"userEmail":"cursor@example.com","subscriptionTier":"pro"}' ;;
models) [ -e '`+down+`' ] && { echo 'network error' >&2; exit 1; }
	printf 'Available models\n\nauto - Auto (default)\ncomposer-2.5 - Composer 2.5\ngpt-5.2 - GPT-5.2\ngpt-5.5 - GPT-5.5\n' ;;
*) exit 1 ;;
esac
`)
	if err := os.Chmod(cli, 0o755); err != nil {
		t.Fatal(err)
	}
	old := provider.CursorExecutable
	provider.CursorExecutable = func() string { return cli }
	provider.ForgetAccounts()
	t.Cleanup(func() {
		provider.CursorExecutable = old
		provider.ForgetAccounts()
	})
	// signed in once `about` has answered: a look waits 3s for it, which
	// a loaded machine's shell can take longer than, and an ask past its
	// own 10s (a new script's first run, under load) is asked again only a
	// minute later, so it is asked afresh here once that 10s is over
	asked := time.Now()
	for deadline := asked.Add(2 * time.Minute); ; time.Sleep(20 * time.Millisecond) {
		if p, err := provider.Find("cursor"); err == nil && p.Account != nil && p.Ready() {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the fake cursor-agent's account never signed in")
		}
		if time.Since(asked) > 15*time.Second {
			provider.ForgetAccounts()
			asked = time.Now()
		}
	}
	// a cold models.dev sync answered here: no test request leaves this
	// process
	client := http.DefaultClient
	http.DefaultClient = &http.Client{Transport: modelsTransport{}}
	t.Cleanup(func() { http.DefaultClient = client })
	return down
}

// Cursor's list exists only at Cursor: the terminal asks for it at start
// and fills a model picker opened before it came, with no m on the
// Providers page (#1789, reyoung: Routing offered cursor/auto alone).
func TestTUIFetchesSubscriptionModels(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fake cursor-agent is a shell script")
	}
	for _, view := range []string{"routing", "routing with catalog sync", "routing with catalog sync last", "provider", "classifier", "agent"} {
		t.Run(view, func(t *testing.T) {
			fakeCursor(t, home(t))
			m := model{w: 120, h: 40, syncing: strings.Contains(view, "sync")}
			init := m.Init()
			m, want := openModelPicker(t, m, view)
			m = typeIn(m, "auto")
			// GPT-5.5 is served by b too, so the list fetched adds a group
			// sorting before cursor/auto under this filter: the model on
			// the cursor stays, not its row
			selected, _ := m.pk.choice()
			if selected == "" {
				t.Fatalf("Cursor's fallback Auto is not in the picker\n%s", m.View())
			}
			m, _ = runModelsCmd(m, init, strings.HasSuffix(view, "last"))
			if got, _ := m.pk.choice(); got != selected || m.pk.input.Value() != "auto" || m.mode != modePick {
				t.Fatalf("the list arriving changed the open picker: choice %q, filter %q, mode %v", got, m.pk.input.Value(), m.mode)
			}
			m.pk.input.SetValue("composer-2.5")
			m.pk.refilter()
			if got, _ := m.pk.choice(); got != want {
				t.Fatalf("Cursor's picker left on Auto alone: got %q, want %q\n%s", got, want, m.View())
			}
		})
	}
}

// A list that couldn't be had at start (no network yet) is asked for again
// while the terminal stays open, not only after a restart.
func TestTUIAsksAgainForAMissingList(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fake cursor-agent is a shell script")
	}
	down := fakeCursor(t, home(t))
	write(t, down, "")
	old := modelsRetry
	modelsRetry = 0
	t.Cleanup(func() { modelsRetry = old })

	m, _ := openModelPicker(t, model{w: 120, h: 40}, "routing")
	m, retry := runModelsCmd(m, m.Init(), false)
	if retry == nil {
		t.Fatal("Cursor's list failed and nothing asks for it again")
	}
	if err := os.Remove(down); err != nil {
		t.Fatal(err)
	}
	provider.ForgetAccounts() // the minute FetchNew waits after a failure
	m, again := runModelsCmd(m, retry, false)
	m.pk.input.SetValue("composer-2.5")
	m.pk.refilter()
	if got, _ := m.pk.choice(); got != "cursor/composer-2.5" {
		t.Fatalf("asked again, still no Composer: %q\n%s", got, m.View())
	}
	if again != nil {
		t.Fatal("asks again with every list there")
	}
}

// openModelPicker opens one of the terminal's four pickers of models, and
// says what Composer 2.5 is called in it.
func openModelPicker(t *testing.T, m model, view string) (model, string) {
	t.Helper()
	switch view {
	case "provider":
		m.page = pageProviders
		m.reloadProviders()
		for i, p := range m.provs {
			if p.ID == "cursor" {
				m.prow = i
			}
		}
		m.openProviderModels("cursor")
		return m, "composer-2.5"
	case "classifier":
		m.page = pageGroups
		m.openClassifier(provider.Group{ID: "test", Name: "Test"})
	case "agent":
		a, err := agent.Find("codex")
		if err != nil {
			t.Fatal(err)
		}
		m.agents = []*agent.Agent{a}
		m.reload()
		m.openFieldPicker()
	default:
		m.page = pageGroups
		m.openMemberPicker(pickMsg{newGroup: "Test"})
	}
	return m, "cursor/composer-2.5"
}

// runModelsCmd runs what Init or a retry returned and hands its messages to
// the model, as the program would, held until the picker is open to stand
// for a vendor answering late; reverse delivers a batch's last first. It
// returns the command the models' message left, the retry, if any.
func runModelsCmd(m model, cmd tea.Cmd, reverse bool) (model, tea.Cmd) {
	if cmd == nil {
		return m, nil
	}
	msg := cmd()
	if batch, ok := msg.(tea.BatchMsg); ok {
		var left tea.Cmd
		for i := range batch {
			j := i
			if reverse {
				j = len(batch) - 1 - i
			}
			var c tea.Cmd
			if m, c = runModelsCmd(m, batch[j], reverse); c != nil {
				left = c
			}
		}
		return m, left
	}
	next, left := m.Update(msg)
	return next.(model), left
}

type modelsTransport struct{}

func (modelsTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	if r.URL.String() != "https://models.dev/api.json" {
		return nil, fmt.Errorf("unexpected request to %s", r.URL.Host)
	}
	return &http.Response{StatusCode: http.StatusOK, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"openai":{"id":"openai","models":{}}}`)), Request: r}, nil
}
