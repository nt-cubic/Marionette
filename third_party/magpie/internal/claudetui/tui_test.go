package claudetui

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
)

// The test binary is also the shim (MAGPIE_TUI_TEST=shim) and a fake
// Claude Code (MAGPIE_TUI_TEST=claude): interactive, it shows a prompt,
// writes each line typed into its transcript as a turn answered "echo:
// <line>"; with -p it answers each stream-json line with a -p result.
func TestMain(m *testing.M) {
	switch os.Getenv("MAGPIE_TUI_TEST") {
	case "shim":
		if err := Run(os.Args[1:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		os.Exit(0)
	case "claude":
		fakeClaude(os.Args[1:])
		os.Exit(0)
	}
	os.Exit(m.Run())
}

// fakeTrust asks whether to trust the folder as Claude Code 2.1.314 does:
// its words placed by cursor moves, "No, exit" chosen, and drawn again on
// No once it listens, the keys before that lost. Enter on No exits.
func fakeTrust(r *bufio.Reader) {
	var mu sync.Mutex
	yes, deaf := false, true
	draw := func() {
		marks := map[bool]string{true: "\x1b[38;2;177;185;249m❯\x1b[39m", false: " "}
		fmt.Printf("\x1b[2GDo\x1b[5Gyou\x1b[9Gtrust\x1b[15Gthis\x1b[20Gfolder?\r\n\x1b[2G%s\x1b[4GNo,\x1b[8Gexit\r\n\x1b[2G%s\x1b[4GYes,\x1b[9GI\x1b[11Gtrust\x1b[17Gthis\x1b[22Gfolder\r\n", marks[!yes], marks[yes])
	}
	draw()
	time.AfterFunc(150*time.Millisecond, func() {
		mu.Lock()
		defer mu.Unlock()
		deaf, yes = false, false
		draw()
	})
	for {
		c, err := r.ReadByte()
		if err != nil {
			os.Exit(0)
		}
		if c == 0x1b {
			seq := make([]byte, 2)
			if _, err := io.ReadFull(r, seq); err != nil {
				os.Exit(0)
			}
			c = seq[1]
		}
		mu.Lock()
		switch {
		case deaf:
		case c == 'B' || c == 'A':
			yes = c == 'B'
			draw()
		case c == '\r' && !yes:
			os.Exit(1)
		case c == '\r':
			mu.Unlock()
			return
		}
		mu.Unlock()
	}
}

func fakeClaude(args []string) {
	if slices.Contains(args, "-p") {
		sc := bufio.NewScanner(os.Stdin)
		for sc.Scan() {
			fmt.Printf(`{"type":"result","subtype":"success","is_error":false,"result":"print mode"}` + "\n")
		}
		return
	}
	if os.Getenv("FAKE_FAIL") == "1" {
		fmt.Print("something went wrong\n")
		os.Exit(3)
	}
	if os.Getenv("FAKE_LOGIN") == "1" {
		// signed in for -p alone: the interactive mode asks how to sign in
		fmt.Print("\r\nSelect login method:\r\n\r\n ❯ 1. Claude account with subscription\r\n   2. Anthropic Console account\r\n")
		time.Sleep(time.Minute)
		os.Exit(0)
	}
	if os.Getenv("FAKE_NOTES") == "1" {
		// onboarding not done: the first-run notes, as 2.1.316 draws them,
		// a space a move of the cursor or a no-break space
		fmt.Print("\r\n Security\x1b[1Cnotes:\r\n\r\n 1. Claude can make mistakes\r\n    You should always review Claude's responses\r\n\r\n 2. Due to prompt injection risks, only use it with code you trust\r\n\r\n Press\u00a0Enter\x1b[1Cto\x1b[1Ccontinue…\r\n")
		time.Sleep(time.Minute)
		os.Exit(0)
	}
	sid := ""
	if i := slices.Index(args, "--session-id"); i >= 0 {
		sid = args[i+1]
	}
	dir := filepath.Join(os.Getenv("CLAUDE_CONFIG_DIR"), "projects", "work")
	_ = os.MkdirAll(dir, 0o700)
	// raw, as Claude Code's terminal is: a paste is taken whole, and Enter
	// outside one sends it
	fakeRaw()
	r := bufio.NewReader(os.Stdin)
	if os.Getenv("FAKE_TRUST") == "1" {
		fakeTrust(r)
	}
	// what it was started with, for the test to read
	_ = os.WriteFile(filepath.Join(os.Getenv("CLAUDE_CONFIG_DIR"), "args"), []byte(strings.Join(args, "\n")), 0o600)
	// FAKE_TAP: Bun loads the tap BUN_OPTIONS names, which says so, and
	// each reply is streamed to it before its message is written
	var tapped *os.File
	if os.Getenv("FAKE_TAP") == "1" {
		_, js, _ := strings.Cut(os.Getenv("BUN_OPTIONS"), "--preload ")
		if b, err := os.ReadFile(js); err == nil && string(b) == string(tapJS) {
			tapped, _ = os.OpenFile(os.Getenv(tapEnv), os.O_APPEND|os.O_WRONLY, 0)
		}
		if tapped != nil {
			_, _ = tapped.WriteString(`{"tap":"on"}` + "\n")
		}
	}
	streams := 0
	fmt.Print("\r\n❯ ")
	// the prompt's words; an image file's path pasted alone is read a
	// moment later and becomes "[Image #<n>:<its bytes>]" where the
	// prompt's words end then
	var mu sync.Mutex
	var typed, paste strings.Builder
	pasting, images := false, 0
	for {
		c, err := r.ReadByte()
		if err != nil {
			return
		}
		switch {
		case c == 0x1b:
			seq := make([]byte, 5)
			if _, err := io.ReadFull(r, seq); err != nil {
				return
			}
			pasting = string(seq) == "[200~"
			if pasting {
				paste.Reset()
				continue
			}
			p := paste.String()
			if b, err := os.ReadFile(p); err == nil && strings.HasPrefix(filepath.Base(p), "image-") {
				images++
				n := images
				go func() {
					time.Sleep(200 * time.Millisecond)
					mu.Lock()
					defer mu.Unlock()
					typed.WriteString(fmt.Sprintf("[Image #%d:%s]", n, b))
					fmt.Printf("[Image #%d]", n)
				}()
				continue
			}
			mu.Lock()
			typed.WriteString(p)
			mu.Unlock()
			fmt.Print(p) // drawn in the prompt
			continue
		case pasting:
			paste.WriteByte(c)
			continue
		case c != '\r':
			mu.Lock()
			typed.WriteByte(c)
			mu.Unlock()
			fmt.Printf("%c", c)
			continue
		}
		mu.Lock()
		line := typed.String()
		typed.Reset()
		mu.Unlock()
		if tapped != nil {
			streams++
			n := streams
			for _, e := range []string{`{"type":"message_start","message":{"id":"msg_1","model":"claude-haiku","usage":{"input_tokens":3}}}`,
				`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"echo: "}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":` + strconvQuote(line) + `}}`,
				`{"type":"content_block_stop","index":0}`,
				`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}`,
				`{"type":"message_stop"}`} {
				_, _ = fmt.Fprintf(tapped, `{"s":%d,"e":%s}`+"\n", n, e)
			}
			_, _ = fmt.Fprintf(tapped, `{"s":%d,"end":"done"}`+"\n", n)
		}
		f, _ := os.OpenFile(filepath.Join(dir, sid+".jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
		rows := []any{
			map[string]any{"type": "user", "message": map[string]any{"role": "user", "content": line}},
			map[string]any{"type": "assistant", "requestId": "req_1", "message": map[string]any{"id": "msg_1", "model": "claude-haiku", "role": "assistant",
				"content": []any{map[string]any{"type": "text", "text": "echo: " + line}}}},
			map[string]any{"type": "system", "subtype": "turn_duration"},
		}
		for _, row := range rows {
			b, _ := json.Marshal(row)
			_, _ = f.Write(append(b, '\n'))
		}
		f.Close()
		fmt.Print("\r\n❯ ")
	}
}

// shim runs the shim on the fake Claude Code with pargs, says lines on its
// input and returns what it wrote, once it wrote want lines.
func runShim(t *testing.T, env []string, pargs []string, lines []string, want int) []map[string]any {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	home := t.TempDir()
	cfg := filepath.Join(home, "claude")
	cmd := exec.Command(exe, pargs...)
	tmp := filepath.Join(home, "tmp")
	_ = os.Mkdir(tmp, 0o700)
	cmd.Env = append(os.Environ(), "MAGPIE_TUI_TEST=shim", "HOME="+home, "XDG_CACHE_HOME="+filepath.Join(home, "cache"), "LOCALAPPDATA="+filepath.Join(home, "cache"),
		"TMPDIR="+tmp, "TMP="+tmp, "TEMP="+tmp, "CLAUDE_CONFIG_DIR="+cfg)
	cmd.Env = append(cmd.Env, env...)
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	t.Setenv("MAGPIE_TUI_TEST_CFG", cfg)
	for _, l := range lines {
		// a line that is JSON is the message's content blocks
		var content any = l
		if json.Valid([]byte(l)) {
			content = json.RawMessage(l)
		}
		b, _ := json.Marshal(map[string]any{"type": "user", "message": map[string]any{"role": "user", "content": content}})
		_, _ = stdin.Write(append(b, '\n'))
	}
	got := make(chan map[string]any)
	go func() {
		sc := bufio.NewScanner(stdout)
		for sc.Scan() {
			var m map[string]any
			_ = json.Unmarshal(sc.Bytes(), &m)
			got <- m
		}
	}()
	var out []map[string]any
	by := time.After(30 * time.Second)
	for len(out) < want {
		select {
		case m := <-got:
			out = append(out, m)
		case <-by:
			t.Fatalf("the shim wrote %d lines of %d in 30s: %v", len(out), want, out)
		}
	}
	return out
}

func strconvQuote(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

func fakePath(t *testing.T) string {
	t.Helper()
	exe, _ := os.Executable()
	// the fake is the test binary, told so by its environment: a script
	// beside it starts it so
	p, script := fakeScript(t.TempDir(), exe)
	if err := os.WriteFile(p, []byte(script), 0o700); err != nil {
		t.Fatal(err)
	}
	return p
}

var pArgs = []string{"-p", "--output-format", "stream-json", "--input-format", "stream-json", "--include-partial-messages", "--verbose",
	"--model", "haiku", "--tools", "WebSearch", "--strict-mcp-config", "--mcp-config", "{}", "--setting-sources", "",
	"--dangerously-skip-permissions", "--effort", "high", "--thinking-display", "summarized"}

func TestShimTurnsInteractive(t *testing.T) {
	out := runShim(t, nil, append([]string{fakePath(t)}, pArgs...), []string{"hello", "/help"}, 5)
	if out[0]["type"] != "system" || out[0]["subtype"] != "init" || out[0]["session_id"] == "" {
		t.Fatalf("first line %v, want the init", out[0])
	}
	var texts []string
	results := 0
	for _, m := range out[1:] {
		switch m["type"] {
		case "assistant":
			b, _ := json.Marshal(m["message"])
			texts = append(texts, string(b))
		case "result":
			results++
		}
	}
	if len(texts) != 2 || !strings.Contains(texts[0], "echo: hello") || results != 2 {
		t.Fatalf("got %v", out)
	}
	// a slash opening the message is pasted after a line break: words,
	// not a command
	if !strings.Contains(texts[1], `\n/help"`) {
		t.Fatalf("the second turn said %s", texts[1])
	}
	args, _ := os.ReadFile(filepath.Join(os.Getenv("MAGPIE_TUI_TEST_CFG"), "args"))
	got := strings.Split(string(args), "\n")
	for _, no := range []string{"-p", "--dangerously-skip-permissions", "--output-format", "--verbose", "--thinking-display"} {
		if slices.Contains(got, no) {
			t.Errorf("the interactive Claude Code was given %s: %q", no, got)
		}
	}
	for _, yes := range []string{"--no-chrome", "mcp__magpie,WebSearch", "--session-id", "high"} {
		if !slices.Contains(got, yes) {
			t.Errorf("the interactive Claude Code wasn't given %s: %q", yes, got)
		}
	}
}

// A message's images are pasted as their files' paths, each alone and
// after the prompt shows the one before: the words and images reach
// Claude Code in their order. One it can't take as a file is an error.
func TestShimPastesImages(t *testing.T) {
	b64 := func(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }
	msg := `[{"type":"text","text":"look:"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"` + b64("red") + `"}},` +
		`{"type":"text","text":"and"},{"type":"image","source":{"type":"base64","media_type":"image/gif","data":"` + b64("blue") + `"}},{"type":"text","text":"now"}]`
	byURL := `[{"type":"text","text":"see"},{"type":"image","source":{"type":"url","url":"https://example.com/a.png"}}]`
	out := runShim(t, nil, append([]string{fakePath(t)}, pArgs...), []string{msg, byURL}, 4)
	var said string
	var failed bool
	for _, m := range out[1:] {
		switch m["type"] {
		case "assistant":
			b, _ := json.Marshal(m["message"])
			said = string(b)
		case "result":
			failed = failed || m["is_error"] == true && strings.Contains(fmt.Sprint(m["result"]), "an image by url")
		}
	}
	if want := `echo: look:\n\n[Image #1:red]\n\nand\n\n[Image #2:blue]\n\nnow"`; !strings.Contains(said, want) {
		t.Fatalf("Claude Code was sent %s, want %s", said, want)
	}
	if !failed {
		t.Fatalf("an image by its URL wasn't an error: %v", out)
	}
	// Claude Code has them once their message is in the transcript: the
	// files are gone while the helper goes on
	left, _ := filepath.Glob(filepath.Join(filepath.Dir(os.Getenv("MAGPIE_TUI_TEST_CFG")), "tmp", imagesPrefix+"*"))
	if len(left) > 0 {
		t.Fatalf("the images are left after their message: %v", left)
	}
}

// A folder of images or a tap's a killed helper left is removed by a
// later one after imagesLeft, and one in use isn't.
func TestDropLeft(t *testing.T) {
	tmp := t.TempDir()
	t.Setenv("TMPDIR", tmp)
	t.Setenv("TMP", tmp)
	t.Setenv("TEMP", tmp)
	for _, prefix := range []string{imagesPrefix, tapPrefix} {
		old, now := filepath.Join(tmp, prefix+"1-a"), filepath.Join(tmp, prefix+"2-b")
		for _, d := range []string{old, now} {
			if err := os.MkdirAll(d, 0o700); err != nil {
				t.Fatal(err)
			}
			_ = os.WriteFile(filepath.Join(d, "image-1.png"), []byte("x"), 0o600)
		}
		then := time.Now().Add(-imagesLeft - time.Minute)
		if err := os.Chtimes(old, then, then); err != nil {
			t.Fatal(err)
		}
		dropLeft()
		if _, err := os.Stat(old); !os.IsNotExist(err) {
			t.Errorf("the %s folder left %s ago is still there (%v)", prefix, imagesLeft, err)
		}
		if _, err := os.Stat(now); err != nil {
			t.Errorf("the %s folder in use was removed: %v", prefix, err)
		}
	}
}

func TestShimFallsBackToPrint(t *testing.T) {
	for _, c := range []struct {
		name   string
		env    []string
		broken bool
	}{
		{"failed start", []string{"FAKE_FAIL=1"}, true},
		{"untrusted folder", []string{"FAKE_TRUST=1"}, false},
		// at once, not after readyWait: runShim waits 30s (#4504)
		{"asks to sign in", []string{"FAKE_LOGIN=1"}, true},
		{"first-run notes", []string{"FAKE_NOTES=1"}, true},
	} {
		t.Run(c.name, func(t *testing.T) {
			out := runShim(t, c.env, append([]string{fakePath(t)}, pArgs...), []string{"hello"}, 1)
			if out[0]["result"] != "print mode" {
				t.Fatalf("got %v, want the -p run's answer", out)
			}
			home := filepath.Dir(os.Getenv("MAGPIE_TUI_TEST_CFG"))
			m, _ := filepath.Glob(filepath.Join(home, "*", "*", "magpie", "claude-tui-broken"))
			m2, _ := filepath.Glob(filepath.Join(home, "*", "magpie", "claude-tui-broken"))
			if got := len(m)+len(m2) > 0; got != c.broken {
				t.Fatalf("marked broken: %v, want %v", got, c.broken)
			}
		})
	}
}

// runOnly runs the shim with OnlyEnv, in home, told "hello", until it
// exits or answers: what it wrote, its stderr, and how it ended.
func runOnly(t *testing.T, home string, env []string) (out, stderr string, err error) {
	t.Helper()
	exe, _ := os.Executable()
	cfg := filepath.Join(home, "claude")
	tmp := filepath.Join(home, "tmp")
	_ = os.Mkdir(tmp, 0o700)
	cmd := exec.Command(exe, append([]string{fakePath(t)}, pArgs...)...)
	cmd.Env = append(os.Environ(), "MAGPIE_TUI_TEST=shim", OnlyEnv+"=1", "HOME="+home, "XDG_CACHE_HOME="+filepath.Join(home, "cache"), "LOCALAPPDATA="+filepath.Join(home, "cache"),
		"TMPDIR="+tmp, "TMP="+tmp, "TEMP="+tmp, "CLAUDE_CONFIG_DIR="+cfg)
	cmd.Env = append(cmd.Env, env...)
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	var errb strings.Builder
	cmd.Stderr = &errb
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	b, _ := json.Marshal(map[string]any{"type": "user", "message": map[string]any{"role": "user", "content": "hello"}})
	_, _ = stdin.Write(append(b, '\n'))
	done := make(chan string, 1)
	go func() {
		var all strings.Builder
		sc := bufio.NewScanner(stdout)
		for sc.Scan() {
			all.WriteString(sc.Text() + "\n")
			if strings.Contains(sc.Text(), `"type":"result"`) {
				break
			}
		}
		done <- all.String()
	}()
	select {
	case out = <-done:
	case <-time.After(30 * time.Second):
		t.Fatal("the shim neither answered nor ended in 30s")
	}
	if !strings.Contains(out, `"type":"result"`) {
		err = cmd.Wait()
	}
	return out, errb.String(), err
}

// With OnlyEnv (the user's "Interactive only"), an interactive mode that
// doesn't come up ends the run, saying why, and never becomes -p; a
// machine marked broken tries it again.
func TestShimOnlyNeverPrints(t *testing.T) {
	for _, c := range []struct {
		name string
		env  []string
	}{
		{"failed start", []string{"FAKE_FAIL=1"}},
		{"untrusted folder", []string{"FAKE_TRUST=1"}},
		{"asks to sign in", []string{"FAKE_LOGIN=1"}},
		{"first-run notes", []string{"FAKE_NOTES=1"}},
	} {
		t.Run(c.name, func(t *testing.T) {
			out, stderr, err := runOnly(t, t.TempDir(), c.env)
			if err == nil || strings.Contains(out, "print mode") {
				t.Fatalf("ended %v, wrote %q", err, out)
			}
			if !strings.Contains(stderr, "doesn't fall back to -p") || !strings.Contains(stderr, "magpie is set to use only it") {
				t.Fatalf("stderr %q", stderr)
			}
		})
	}
	t.Run("marked broken", func(t *testing.T) {
		home := t.TempDir()
		t.Setenv("HOME", home)
		t.Setenv("XDG_CACHE_HOME", filepath.Join(home, "cache"))
		t.Setenv("LOCALAPPDATA", filepath.Join(home, "cache"))
		MarkBroken()
		if _, ok := BrokenSince(); !ok {
			t.Fatal("no mark")
		}
		out, stderr, err := runOnly(t, home, nil)
		if err != nil || strings.Contains(out, "print mode") || !strings.Contains(out, "echo: hello") {
			t.Fatalf("ended %v, wrote %q, stderr %q", err, out, stderr)
		}
	})
}

// Each failed start in a row keeps runs on -p longer, to a day; one that
// shows its prompt forgets them.
func TestBrokenGrows(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, "cache"))
	t.Setenv("LOCALAPPDATA", filepath.Join(home, "cache"))
	for n, want := range map[int]time.Duration{1: 30 * time.Minute, 2: time.Hour, 3: 2 * time.Hour, 6: 16 * time.Hour, 7: 24 * time.Hour, 40: 24 * time.Hour} {
		if got := brokenFor(n); got != want {
			t.Errorf("brokenFor(%d) = %s, want %s", n, got, want)
		}
	}
	if _, ok := BrokenSince(); ok {
		t.Fatal("broken with no mark")
	}
	MarkBroken()
	MarkBroken()
	if _, n, _ := brokenRead(); n != 2 {
		t.Fatalf("count %d, want 2", n)
	}
	// 45 minutes ago: past one failure's 30 minutes, within two's hour
	old := time.Now().Add(-45 * time.Minute)
	_ = os.Chtimes(broken(), old, old)
	if _, ok := BrokenSince(); !ok {
		t.Fatal("two failures 45 minutes ago went back to the interactive mode")
	}
	_ = os.WriteFile(broken(), nil, 0o600) // a mark from before the count
	_ = os.Chtimes(broken(), old, old)
	if _, ok := BrokenSince(); ok {
		t.Fatal("an old mark 45 minutes ago still holds")
	}
	clearBroken()
	if _, _, ok := brokenRead(); ok {
		t.Fatal("cleared, the mark stayed")
	}
}

// A start that shows its prompt removes the mark of earlier failures, so
// the next failure counts from one.
func TestShimStartClearsBroken(t *testing.T) {
	home := t.TempDir()
	env := []string{"HOME=" + home, "XDG_CACHE_HOME=" + filepath.Join(home, "cache"), "LOCALAPPDATA=" + filepath.Join(home, "cache")}
	for _, e := range env {
		k, v, _ := strings.Cut(e, "=")
		t.Setenv(k, v)
	}
	// three failures, the last a day and a half ago: past its 2 hours
	MarkBroken()
	MarkBroken()
	MarkBroken()
	old := time.Now().Add(-36 * time.Hour)
	_ = os.Chtimes(broken(), old, old)
	out := runShim(t, env, append([]string{fakePath(t)}, pArgs...), []string{"hello"}, 3)
	if out[0]["subtype"] != "init" {
		t.Fatalf("got %v, want an interactive turn", out)
	}
	if _, _, ok := brokenRead(); ok {
		t.Fatal("a start that showed its prompt left the mark")
	}
}

func TestShimTrustsWhenTold(t *testing.T) {
	out := runShim(t, []string{"FAKE_TRUST=1", TrustEnv + "=1"}, append([]string{fakePath(t)}, pArgs...), []string{"hello"}, 3)
	if out[0]["subtype"] != "init" || out[1]["type"] != "assistant" {
		t.Fatalf("got %v, want an interactive turn", out)
	}
}

func TestPasteable(t *testing.T) {
	for _, c := range []struct {
		in   string
		busy bool
		want string
	}{
		{"hello", false, "hello"},
		{"/help", false, "\n/help"},
		{"  !ls", false, "\n!ls"},
		{"# note", true, userMark + "# note"},
		{"a\r\nb\x1b[201~c", false, "a\nb[201~c"},
		{"x / y", true, "x / y"},
	} {
		if got := pasteable(c.in, c.busy); got != c.want {
			t.Errorf("pasteable(%q, %v) = %q, want %q", c.in, c.busy, got, c.want)
		}
	}
}

type lines struct {
	out          []map[string]any
	rows, overed int
}

func (l *lines) emit(v any) {
	b, _ := json.Marshal(v)
	var m map[string]any
	_ = json.Unmarshal(b, &m)
	l.out = append(l.out, m)
}
func (l *lines) sawRow()   { l.rows++ }
func (l *lines) turnOver() { l.overed++ }

func TestTranslatorAPIError(t *testing.T) {
	l := &lines{}
	tr := newTranslator(l)
	for _, row := range []string{
		`{"type":"user","message":{"role":"user","content":"hi"}}`,
		`{"type":"assistant","isApiErrorMessage":true,"apiErrorStatus":429,"apiErrorParams":{"rate_limit_info":{"status":"rejected"}},"message":{"id":"x","model":"<synthetic>","content":[{"type":"text","text":"You've hit your limit"}]}}`,
		`{"type":"system","subtype":"turn_duration"}`,
		`{"type":"user","isSidechain":true,"message":{"role":"user","content":"agent"}}`,
	} {
		tr.row([]byte(row))
	}
	kinds := []string{}
	for _, m := range l.out {
		kinds = append(kinds, m["type"].(string))
	}
	if !slices.Equal(kinds, []string{"assistant", "rate_limit_event", "result"}) || l.rows != 1 || l.overed != 2 {
		t.Fatalf("got %v (rows %d, over %d)", l.out, l.rows, l.overed)
	}
	if r := l.out[2]; r["is_error"] != true || r["result"] != "You've hit your limit" || r["api_error_status"] != float64(429) {
		t.Fatalf("result %v", r)
	}
}

// A Claude Code in WSL is pasted an image's path as the distro reads it,
// and is handed TERM through WSLENV, the gateway's own names kept.
func TestShimInWSL(t *testing.T) {
	s := &shim{wsl: &WSL{Args: []string{"-d", "Ubuntu"}, Mount: "/mnt/", Projects: `\\wsl$\Ubuntu\home\me\.claude\projects`}}
	if got := s.path(`C:\Users\me\AppData\Local\Temp\magpie-images-1\1.png`); got != "/mnt/c/Users/me/AppData/Local/Temp/magpie-images-1/1.png" {
		t.Fatalf("path %q", got)
	}
	if got := (&shim{}).path("/tmp/a.png"); got != "/tmp/a.png" {
		t.Fatalf("this machine's path %q", got)
	}
	t.Setenv(WSLEnv, `{"args":["-d","Ubuntu"],"projects":"x"}`)
	t.Setenv("WSLENV", "CLAUDE_CONFIG_DIR/p")
	env := childEnv()
	var wslenv []string
	for _, e := range env {
		if strings.HasPrefix(e, WSLEnv+"=") {
			t.Fatalf("the child was handed %s", e)
		}
		if strings.HasPrefix(strings.ToUpper(e), "WSLENV=") {
			wslenv = append(wslenv, e)
		}
	}
	if len(wslenv) != 1 || wslenv[0] != "WSLENV=CLAUDE_CONFIG_DIR/p:TERM" {
		t.Fatalf("WSLENV %q", wslenv)
	}
	// the tap's go through WSLENV too, in place of the shim's own
	t.Setenv("BUN_OPTIONS", "--smol")
	env = childEnv("BUN_OPTIONS=--preload /mnt/c/t/tap.js", tapEnv+"=/mnt/c/t/tap.jsonl")
	var bun []string
	for _, e := range env {
		if strings.HasPrefix(e, "BUN_OPTIONS=") || strings.HasPrefix(e, tapEnv+"=") || strings.HasPrefix(e, "WSLENV=") {
			bun = append(bun, e)
		}
	}
	if want := []string{"WSLENV=CLAUDE_CONFIG_DIR/p:TERM:BUN_OPTIONS:" + tapEnv, "BUN_OPTIONS=--preload /mnt/c/t/tap.js", tapEnv + "=/mnt/c/t/tap.jsonl"}; !slices.Equal(bun, want) {
		t.Fatalf("env %q, want %q", bun, want)
	}
}
