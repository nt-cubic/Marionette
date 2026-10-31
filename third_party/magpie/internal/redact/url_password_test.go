package redact

import (
	"regexp"
	"strings"
	"testing"
)

// xuanwo on X, 10-29: "很多项目名被识别成了 password". The geodatafusion
// repo's PostGIS test runs against
// postgresql://geodatafusion:geodatafusion@127.0.0.1:55432/geodatafusion
// (testing/postgis/README.md, .github/workflows/postgis-differential.yml).
// The URL's password was masked, so the word was known, and the project's
// name went as a made-up word in every path, crate and command of the
// Codex summary in their screenshot.

// the README's and the workflow's lines, as the repo has them
const geoURLLines = "GEODATAFUSION_POSTGIS_URL=postgresql://geodatafusion:geodatafusion@127.0.0.1:55432/geodatafusion \\\n" +
	"  GEODATAFUSION_POSTGIS_URL: postgresql://geodatafusion:geodatafusion@127.0.0.1:55432/geodatafusion\n"

// the summary in the screenshot, as Codex wrote it
const geoSummary = "**当前状态**\n" +
	"- 工作树: `/Users/xuanwo/Code/worktrees/f745/geodatafusion`\n" +
	"- 分支: `xuanwo/sqllogictest-coverage`\n" +
	"- 构建固定使用 `CARGO_TARGET_DIR=/Users/xuanwo/.cache/geodatafusion-goal/target`、`CARGO_BUILD_JOBS=4`、`CARGO_INCREMENTAL=0`\n" +
	"**未提交改动**\n" +
	"- `rust/geodatafusion/tests/geos_runtime.rs`: 新增 GEOS runtime 最低版本断言; 设置 `GEODATAFUSION_EXPECT_GEOS_VERSION` 时要求精确版本。\n" +
	"- `geodatafusion.toml`: `ST_LineMerge`、`ST_ConcaveHull` 从 verified 降为 partial\n" +
	"```bash\n" +
	"export GEOS_LIB_DIR=/Users/xuanwo/.cache/geodatafusion-goal/geos-3.11.5-prefix/lib\n" +
	"CARGO_TARGET_DIR=/Users/xuanwo/.cache/geodatafusion-goal/target \\\n" +
	"  cargo test -p geodatafusion --features geos-3_11 --locked --lib --no-run\n" +
	"BIN=$(ls -t /Users/xuanwo/.cache/geodatafusion-goal/target/debug/deps/geodatafusion-* | grep -v '\\.d$' | head -1)\n" +
	"```\n"

func TestURLPasswordWrittenElsewhereGoesAsWritten(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	o := Options{Secrets: true, Personal: true, Shape: true}

	// the URL itself: the password is its user and its database, so
	// nothing of it is hidden by masking it
	if out, n := Mask(geoURLLines, o); out != geoURLLines || n != 0 {
		t.Errorf("%d masked: %q", n, out)
	}
	// and so the project's name is not known: the summary goes as written,
	// as a request's text and as a tool call's arguments
	if out, n := Mask(geoSummary, o); out != geoSummary || n != 0 {
		t.Errorf("%d masked in the summary: %q", n, out)
	}
	body := `{"input":[{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"psql postgresql://geodatafusion:geodatafusion@127.0.0.1:55432/geodatafusion -c 'select 1'\"}","call_id":"call_1"},` +
		`{"type":"message","role":"user","content":[{"type":"input_text","text":"cargo test -p geodatafusion"}]}]}`
	if out, n := MaskJSON([]byte(body), o); string(out) != body || n != 0 {
		t.Errorf("%d masked in a body: %s", n, out)
	}

	// the same for a password that is another part of its URL: a user's,
	// a host's label, the database
	for _, in := range []string{
		"postgres://postgres:postgres@localhost:5432/app",
		"mysql://root:root@127.0.0.1/shop",
		"amqp://guest:guest@rabbit:5672/",
		"mysql://app:shopdb@db.internal/shopdb?ssl=true",
		"redis://:cachebox@cachebox:6379/0",
		`"url": "postgresql://geo:geo@db/geo"`,
	} {
		if out, n := Mask(in, o); out != in || n != 0 {
			t.Errorf("%d masked: %q went as %q", n, in, out)
		}
	}
}

// A password not written elsewhere in its URL is a password still, of
// whatever look: masked there, and as known wherever it is next. A word
// that is also a project's name is masked with it then: that is the
// user's password, and Settings › Privacy's Passwords row (Warn) or
// Never mask lets it go.
func TestURLPasswordStillMasked(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	o := Options{Secrets: true, Personal: true}
	ph := `\{\{PASSWORD_[a-z2-7]{8}\}\}`
	for _, c := range []struct{ in, want string }{
		{"postgres://app:hunter2pass@db.acme-corp.cn:5432/x", `^postgres://app:` + ph + `@db\.acme-corp\.cn:5432/x$`},
		{"postgresql://geo:opensesame@127.0.0.1:55432/geodata", `^postgresql://geo:` + ph + `@127\.0\.0\.1:55432/geodata$`},
		// a part of a part is no part: the user geo, the database geodb
		{"postgresql://geo:geod@127.0.0.1/geodb", `^postgresql://geo:` + ph + `@127\.0\.0\.1/geodb$`},
		{"REDIS_URL=redis://:r3disPass@cache.acme-corp.cn:6379/0", `^REDIS_URL=redis://:` + ph + `@cache\.acme-corp\.cn:6379/0$`},
	} {
		if out, _ := Mask(c.in, o); !regexp.MustCompile(c.want).MatchString(out) {
			t.Errorf("%q went as %q", c.in, out)
		}
	}
	// in a tool call's arguments too
	body := `{"input":[{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"psql postgresql://geo:Tr0ub4dor3@10.0.0.5/geodata\"}","call_id":"call_1"}]}`
	if out, n := MaskJSON([]byte(body), o); strings.Contains(string(out), "Tr0ub4dor3") || n != 1 {
		t.Fatalf("%d masked in a tool call: %s", n, out)
	}
	// known: the word goes masked in other words afterwards
	if out, _ := Mask("cd ~/src/opensesame && ls", o); strings.Contains(out, "opensesame") {
		t.Fatalf("a known password went as written: %q", out)
	}
}
