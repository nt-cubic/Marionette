package edit

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/pelletier/go-toml/v2/unstable"
)

// tomlShaper finds every key and comment where the parser's own Shape does.
func TestTOMLShaperMatchesParser(t *testing.T) {
	doc := "# top\na = 1 # one\n\n[t]\n  b.c = \"x#y\"  # two\n\"q\" = [\n  1,\n]\n[[arr]]\nz = { k = 1 }\n"
	lines := splitLines(doc)
	shapeOf := tomlShaper(lines)
	p := unstable.Parser{KeepComments: true}
	p.Reset([]byte(strings.Join(lines, "\n")))
	n := 0
	check := func(it *unstable.Node) {
		if it.Raw.Length == 0 {
			return
		}
		if got, want := shapeOf(it.Raw), p.Shape(it.Raw); got != want {
			t.Errorf("%v %q: %+v, the parser says %+v", it.Kind, p.Raw(it.Raw), got, want)
		}
		n++
	}
	for p.NextExpression() {
		e := p.Expression()
		check(e)
		if e.Kind != unstable.Comment {
			for k := e.Key(); k.Next(); {
				check(k.Node())
			}
		}
		if c := e.Next(); c != nil {
			check(c)
		}
	}
	if err := p.Error(); err != nil || n < 8 {
		t.Fatalf("parsed %d nodes: %v", n, err)
	}
}

// A big config.toml is parsed in a time that grows with its size, not its
// square: Codex's own reached 1.5 MB, and each key's line was counted from
// the top of the file, a third of a second at every magpie start.
func TestTOMLBigFileParsesInLinearTime(t *testing.T) {
	var b strings.Builder
	for i := 0; b.Len() < 3<<20; i++ {
		fmt.Fprintf(&b, "[projects.\"/Users/me/p%d\"]\ntrust_level = \"trusted\" # note\n\n", i)
	}
	b.WriteString("model_provider = \"magpie\"\n")
	lines := splitLines("model_provider = \"x\"\n" + b.String())
	start := time.Now()
	f, err := parseTOMLFile(lines)
	if err != nil {
		t.Fatal(err)
	}
	if d := time.Since(start); d > time.Second {
		t.Errorf("a 3 MB file took %v to parse", d)
	}
	if len(f.tables) < 30000 || f.root.keys[0].name != "model_provider" {
		t.Fatalf("parsed %d tables, root %+v", len(f.tables), f.root.keys)
	}
	last := f.tables[len(f.tables)-1]
	if k := last.keys[len(last.keys)-1]; k.name != "model_provider" || k.from != len(lines)-2 && k.from != len(lines)-1 {
		t.Errorf("last key %+v of %d lines", k, len(lines))
	}
}
