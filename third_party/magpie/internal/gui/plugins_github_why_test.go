package gui

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/url"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/ghtoken"
	"github.com/yetone/magpie/internal/plugin"
)

// The Plugins page says why GitHub's list isn't whole in the reader's
// language when magpie knows the kind of why, rather than Go's English in
// the middle of the page (the Kiko03O1 reply on X: "no answer in 8s"
// under 非官方插件 · GitHub). Each kind as the errors arrive: wrapped in
// TaggedMissing, in a url.Error, or as they are.
func TestTaggedWhyKind(t *testing.T) {
	timeout := &url.Error{Op: "Get", URL: "https://raw.githubusercontent.com/a/b/main/package.json", Err: &net.OpError{Op: "dial", Net: "tcp", Err: timeoutErr{}}}
	refused := &url.Error{Op: "Get", URL: "http://127.0.0.1:1/raw", Err: &net.OpError{Op: "dial", Net: "tcp", Err: errors.New("connect: connection refused")}}
	for _, c := range []struct {
		name string
		err  error
		want string
	}{
		{"the page stopped waiting", &plugin.TaggedMissing{Missing: 7, Why: fmt.Errorf("%w in %s", plugin.ErrNoAnswer, 8*time.Second)}, "slow"},
		{"a read past its deadline", &plugin.TaggedMissing{Missing: 2, Why: &url.Error{Op: "Get", URL: "https://raw.githubusercontent.com/x", Err: context.DeadlineExceeded}}, "slow"},
		{"a dial that timed out", &plugin.TaggedMissing{Missing: 1, Why: timeout}, "slow"},
		{"the search past its deadline", fmt.Errorf("search: %w", context.DeadlineExceeded), "slow"},
		{"GitHub's rate limit", ghtoken.Limited{Until: time.Now().Add(time.Hour)}, "limited"},
		{"GitHub's rate limit, missing", &plugin.TaggedMissing{Missing: 3, Why: ghtoken.Limited{Token: true}}, "limited"},
		{"refused: said as it is", &plugin.TaggedMissing{Missing: 12, Why: refused}, ""},
		{"anything else", errors.New("GitHub's search: 502 Bad Gateway"), ""},
	} {
		if got := taggedWhyKind(c.err); got != c.want {
			t.Errorf("%s: %q, want %q (%v)", c.name, got, c.want, c.err)
		}
	}
	// the error as it came is still what the line's hover says
	if e := fmt.Errorf("%w in %s", plugin.ErrNoAnswer, 8*time.Second); e.Error() != "no answer in 8s" {
		t.Errorf("hover: %q", e)
	}
}

type timeoutErr struct{}

func (timeoutErr) Error() string   { return "i/o timeout" }
func (timeoutErr) Timeout() bool   { return true }
func (timeoutErr) Temporary() bool { return true }
