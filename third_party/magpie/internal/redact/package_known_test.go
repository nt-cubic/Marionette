package redact

import (
	"encoding/json"
	"runtime"
	"strings"
	"testing"
)

// dniray on Discord, v0.1.1965, after v0.1.1932's fix: "问题还在", with
// import { and, eq } from ' drizzle-orm ' going out as
// from ' {{SECRET_o2tu7vej}} '. The module an import loads went as
// written only where the model found it on that line: once the model had
// taken the name for a secret anywhere else in a request (package.json's
// dependencies, a tsc error, a sentence), the name was known, and a known
// value is masked wherever it is, on the import line too.
func TestKnownPackageNameGoesAsWrittenWhereCodeLoadsIt(t *testing.T) {
	const line = "import { and, eq } from ' drizzle-orm '"
	f := &fakeModel{values: map[string]string{"drizzle-orm": "secret"}}
	o := Options{Secrets: true, Personal: true, Model: f.find}
	// a sentence the name can't be told apart in, then the import, in one
	// request: Claude Code's tool results as it sends them
	body, _ := json.Marshal(map[string]any{"model": "m", "messages": []any{
		map[string]any{"role": "user", "content": "we keep the drizzle-orm here"},
		map[string]any{"role": "assistant", "content": []any{map[string]any{"type": "tool_use", "id": "toolu_01", "name": "Read", "input": map[string]any{"file_path": "/w/src/db.ts"}}}},
		map[string]any{"role": "user", "content": []any{map[string]any{"type": "tool_result", "tool_use_id": "toolu_01", "content": line + "\n"}}},
	}})
	out, _ := MaskJSON(body, o)
	if !strings.Contains(string(out), line) {
		t.Errorf("the import was masked: %s", out)
	}
	// and in the next request, with the model finding nothing in it
	if out, n := Mask(line, Options{Secrets: true, Personal: true}); n != 0 {
		t.Errorf("as known: %q", out)
	}
	f2 := &fakeModel{values: map[string]string{"drizzle-orm": "secret"}}
	o2 := Options{Secrets: true, Personal: true, Model: f2.find}
	body2, _ := json.Marshal(map[string]any{"messages": []any{map[string]any{"role": "user", "content": line}}})
	if out, _ := MaskJSON(body2, o2); !strings.Contains(string(out), line) {
		t.Errorf("as known, in a body: %s", out)
	}
}

// Where else a package's name is written in what an agent reads: a
// dependency in package.json, Cargo.toml or a lock file, a path under
// node_modules, a compiler's "Cannot find module", an install, a pin at a
// version, and an import with an editor's line numbers or a diff's mark
// before it. The model took it for a secret in each, and it went as a
// placeholder the agent can't resolve.
func TestModelPackageNamesInManifestsAndOutput(t *testing.T) {
	for _, c := range []struct{ text, value string }{
		{"{\n  \"dependencies\": {\n    \"drizzle-orm\": \"^0.36.4\",\n    \"jsonwebtoken\": \"~9.0.2\"\n  }\n}", "drizzle-orm"},
		{"{\"devDependencies\": {\"drizzle-kit\": \"0.28.1\"}}", "drizzle-kit"},
		{"\"jsonwebtoken\": \"workspace:*\",", "jsonwebtoken"},
		{"\"drizzle-orm\": \">=0.30.0 <1\",", "drizzle-orm"},
		{"[dependencies]\njsonwebtoken = \"9\"\n", "jsonwebtoken"},
		{"     1→import drizzle from 'drizzle-orm';", "drizzle-orm"},
		{"     2\timport sql from 'postgres';\n", "postgres"},
		{"     3→import { pgTable } from 'drizzle-orm/pg-core';", "drizzle-orm"},
		{"import { z } from '@acme/drizzle-orm';", "drizzle-orm"},
		{"12: import { sql } from \"drizzle-orm\";", "drizzle-orm"},
		{"+import { and, eq } from ' drizzle-orm '", "drizzle-orm"},
		{"-const jwt = require('jsonwebtoken')", "jsonwebtoken"},
		{"node_modules/drizzle-orm/pg-core/columns/text.d.ts:12:5 - error TS2322", "drizzle-orm"},
		{"src/db.ts:1:27 - error TS2307: Cannot find module 'drizzle-orm' or its corresponding type declarations.", "drizzle-orm"},
		{"Error: Cannot find module 'jsonwebtoken'\nRequire stack:", "jsonwebtoken"},
		{"ModuleNotFoundError: No module named 'secretstorage'", "secretstorage"},
		{"pnpm add drizzle-orm postgres && pnpm add -D drizzle-kit", "drizzle-orm"},
		{"npm install --save jsonwebtoken", "jsonwebtoken"},
		{"pip install django-rest-passwordreset", "django-rest-passwordreset"},
		{"cargo add jsonwebtoken", "jsonwebtoken"},
		{"added drizzle-orm@0.36.4 to dependencies", "drizzle-orm"},
		{"  drizzle-orm@0.36.4:\n    resolution: {}", "drizzle-orm"},
	} {
		f := &fakeModel{values: map[string]string{c.value: "secret"}}
		if out, n := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); n != 0 {
			t.Errorf("%q: masked %q", c.value, out)
		}
	}
}

// A secret beside those shapes is masked still: one the model finds under
// a field that isn't a dependency, after an install's words in prose, and
// a known one in an ordinary line.
func TestPackageShapesKeepSecrets(t *testing.T) {
	for _, c := range []struct{ text, value string }{
		{"{\"password\": \"Zq8vR2mT6wLp\"}", "Zq8vR2mT6wLp"},
		{"{\"Zq8vR2mT6wLp\": \"hunter2\"}", "Zq8vR2mT6wLp"},
		{"the login is Zq8vR2mT6wLp@10.0.0.5", "Zq8vR2mT6wLp"},
		{"     1→const token = 'Zq8vR2mT6wLp';", "Zq8vR2mT6wLp"},
		{"use the code Zq8vR2mT6wLp to install it", "Zq8vR2mT6wLp"},
	} {
		f := &fakeModel{values: map[string]string{c.value: "secret"}}
		if out, _ := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); strings.Contains(out, c.value) {
			t.Errorf("%q left in %q", c.value, out)
		}
	}
	// a known secret in a line that isn't code loading it
	if out, _ := Mask("DB_PASSWORD=Zq8hunterPass42", Options{Secrets: true}); strings.Contains(out, "Zq8hunterPass42") {
		t.Fatalf("not masked to begin with: %q", out)
	}
	for _, s := range []string{"the password from Zq8hunterPass42 there", "     1→const p = 'Zq8hunterPass42';"} {
		if out, _ := Mask(s, Options{Secrets: true}); strings.Contains(out, "Zq8hunterPass42") {
			t.Errorf("known secret left in %q", out)
		}
	}
}

// The words that name a module an error could not load are read where
// they stand, right before the quote — not by reading everything before
// them. A session's body holds a place a known value turns up at for each
// turn it has been in it, and knownSpans asks packageAt at each: reading
// the whole text before the quote there paid the body's length again for
// every value already masked in it, so a long session's turn held minutes
// in the masking and was answered 504 (#4766). What is read is the last
// 64 bytes of it now, as assignedSecret reads the 64 before the value it
// judges.
func TestMissingModuleReadsOnlyTheWordsBeforeTheQuote(t *testing.T) {
	const cue = "Cannot find module "
	const name = "drizzle-orm"
	run := func(s string, a int) bool { return missingModuleAt(s, a, a+len(name)) }
	for _, pad := range []int{10, 200, 200000} {
		s := strings.Repeat("x", pad) + " " + cue + "'" + name + "' or its type declarations"
		if !run(s, len(strings.Repeat("x", pad)+" "+cue)+1) {
			t.Fatalf("%d bytes before it: not named by the error", pad)
		}
	}
	// a value quoted under no such words is not a module
	s := strings.Repeat("x", 200000) + " " + `"drizzle-orm"`
	if run(s, len(strings.Repeat("x", 200000)+" ")) {
		t.Fatal("a plain quoted value named by the error")
	}
	// the words are read as a word's end: one that only ends in them is not
	s = strings.Repeat("x", 200000) + " Cannot find moduleX'" + name + "'"
	if run(s, len(strings.Repeat("x", 200000)+" Cannot find moduleX")+1) {
		t.Fatal("a word that only ends in the error's words named by it")
	}
	// the words stand whole, however long the text before them is
	s = "Python says: ModuleNotFoundError: No module named '" + name + "'"
	if !run(s, len("Python says: ModuleNotFoundError: No module named '")) {
		t.Fatal("No module named: not named by the error")
	}
	s = strings.Repeat("x", 200000) + " Can't resolve '" + name + "'"
	if !run(s, len(strings.Repeat("x", 200000)+" Can't resolve '")) {
		t.Fatal("Can't resolve: not named by the error")
	}
	// a letter a lower case folds wider than in the text before: read as
	// before, and still named
	s = "İSTANBUL " + strings.Repeat("x", 128) + " " + cue + "'" + name + "'"
	if !run(s, len("İSTANBUL "+strings.Repeat("x", 128)+" "+cue)+1) {
		t.Fatal("a wider fold before it: not named by the error")
	}

	// the bytes one call makes: a string of everything before the quote,
	// twice lowered, is what made a long turn hold. Nothing of the size
	// of the text before it is made now.
	s = strings.Repeat("x", 400000) + " " + cue + "'" + name + "'"
	a := len(strings.Repeat("x", 400000)+" "+cue) + 1
	run(s, a) // once, so lazy init is no part of the count
	var m0, m1 runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&m0)
	run(s, a)
	runtime.ReadMemStats(&m1)
	if made := m1.TotalAlloc - m0.TotalAlloc; made > 8<<10 {
		t.Fatalf("judging the words made %d bytes, 8 KiB at most: the text before the quote is being read whole", made)
	}

	// a session written in Chinese is read as cheaply: the text before the
	// words is not read whole because it is not ASCII
	zh := strings.Repeat("找不到模块", 30000) + "：" + cue
	s = zh + "'" + name + "'"
	a = len(zh) + 1
	if !run(s, a) {
		t.Fatal("after Chinese text: not named by the error")
	}
	runtime.GC()
	runtime.ReadMemStats(&m0)
	run(s, a)
	runtime.ReadMemStats(&m1)
	if made := m1.TotalAlloc - m0.TotalAlloc; made > 8<<10 {
		t.Fatalf("after Chinese text, judging the words made %d bytes, 8 KiB at most", made)
	}

	// spaces between the words and the quote, more than the window, still
	// leave the words read
	sp := strings.Repeat("x", 1000) + " " + cue + strings.Repeat(" ", 100)
	if !run(sp+"'"+name+"'", len(sp)+1) {
		t.Fatal("many spaces before the quote: not named by the error")
	}
}
