package gui

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// #1929 (versun): a group whose classifier couldn't say the last time it
// was asked says why on the Routing page's card, rather than only in each
// request's details while its rules with an intent quietly never match. A
// group whose classifier answered, or that has none, says nothing.
func TestGroupsSayAClassifierThatFailed(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Key: "k" + id, Chat: "http://127.0.0.1:1/v1", Models: []string{"m"}}); err != nil {
			t.Fatal(err)
		}
	}
	// MLX-Serve's own words, its decision model asked as a chat model
	const why = `b/m: This is a typed-decision model; it cannot serve chat/text requests. Use POST /v1/decisions instead.`
	at := time.Now().Add(-time.Minute).Truncate(time.Second)
	was := classifierFailure
	t.Cleanup(func() { classifierFailure = was })
	classifierFailure = func(model string) (string, time.Time, bool) {
		if model == "b/m" {
			return why, at, true
		}
		return "", time.Time{}, false
	}
	for _, g := range []provider.Group{
		{ID: "intent", Name: "Intent", Members: []string{"a/m", "b/m"}, Classifier: "b/m", Rules: []provider.Rule{{Use: "b/m", Intent: "planning"}}},
		{ID: "fine", Name: "Fine", Members: []string{"a/m", "b/m"}, Classifier: "a/m", Rules: []provider.Rule{{Use: "b/m", Intent: "planning"}}},
		{ID: "plain", Name: "Plain", Members: []string{"a/m", "b/m"}},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	b, _ := json.Marshal(groupsState())
	var st struct {
		Groups []struct {
			ID               string `json:"id"`
			ClassifierFailed *struct {
				Error string    `json:"error"`
				At    time.Time `json:"at"`
			} `json:"classifierFailed"`
		} `json:"groups"`
	}
	if err := json.Unmarshal(b, &st); err != nil {
		t.Fatal(err)
	}
	seen := 0
	for _, g := range st.Groups {
		switch g.ID {
		case "intent":
			seen++
			if g.ClassifierFailed == nil || g.ClassifierFailed.Error != why || !g.ClassifierFailed.At.Equal(at) {
				t.Fatalf("the failing classifier's group: %+v", g.ClassifierFailed)
			}
		case "fine", "plain":
			seen++
			if g.ClassifierFailed != nil {
				t.Fatalf("%s: %+v", g.ID, g.ClassifierFailed)
			}
		}
	}
	if seen != 3 {
		t.Fatalf("groups: %s", b)
	}
	if strings.Count(string(b), `"classifierFailed"`) != 1 {
		t.Fatalf("classifierFailed sent for a group it isn't for: %s", b)
	}
}
