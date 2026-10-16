package gui

import (
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/redact"
)

// Settings lists the kinds of personal data the gateway masks, in
// REDACT_KINDS, with what each is until the user chooses: the same ids, in
// the same order, with the same defaults as redact.Categories, so a toggle
// shows what is masked.
func TestRedactKindsAreRedacts(t *testing.T) {
	js, err := os.ReadFile("assets/app.js")
	if err != nil {
		t.Fatal(err)
	}
	_, list, _ := strings.Cut(string(js), "\nconst REDACT_KINDS = [")
	list, _, _ = strings.Cut(list, "\n];")
	var got []string
	for _, m := range regexp.MustCompile(`(?m)^  \["(\w+)",.*, (true|false)\],$`).FindAllStringSubmatch(list, -1) {
		got = append(got, m[1]+"="+m[2])
	}
	var want []string
	for _, c := range redact.Categories {
		if c.On {
			want = append(want, c.ID+"=true")
		} else {
			want = append(want, c.ID+"=false")
		}
	}
	if strings.Join(got, " ") != strings.Join(want, " ") {
		t.Fatalf("app.js REDACT_KINDS: %v\nredact.Categories: %v", got, want)
	}
}
