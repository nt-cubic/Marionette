package plugin

import (
	"strings"
	"testing"
)

// A package's magpie.name is its own word on its row (#4558): one short
// line, as written, and nothing that could make it read as another name.
func TestOwnNameBounds(t *testing.T) {
	for said, want := range map[string]string{
		"华为云 HUAWEI":            "华为云 HUAWEI",
		"  OfficeAce \n":        "OfficeAce",
		"":                      "",
		"Two\nlines":            "",
		"Tab\tbed":              "",
		"Qoder‮":                "",
		"zero​width":            "",
		strings.Repeat("a", 80): strings.Repeat("a", 80),
		strings.Repeat("a", 81): "",
		"bad \xff utf-8":        "",
	} {
		if got := ownName(said); got != want {
			t.Errorf("ownName(%q) = %q, want %q", said, got, want)
		}
	}
}
