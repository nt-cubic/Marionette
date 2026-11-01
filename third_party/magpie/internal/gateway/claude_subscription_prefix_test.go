package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/claudetui"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// prefixHarness is a gateway whose Claude subscription runs a stand-in
// for Claude Code that keeps its session as Claude Code does: a line for
// each user message it is told and each reply, with its uuid and the
// session's id. It logs its arguments, the tools its MCP helper offers
// (<pid>.tools) and each line it is told, and answers "pid P had H turn
// T": H the lines its session had as it started (a branch's copy, or a
// resumed session), T the turns it was told. A turn that says CALLTOOL
// calls the client's read tool first and says what the call was answered.
type prefixHarness struct {
	t      *testing.T
	s      *Server
	srv    *httptest.Server
	logs   string
	config string
}

func newPrefixHarness(t *testing.T) *prefixHarness {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("a shell script stands in for Claude Code")
	}
	if _, err := exec.LookPath("curl"); err != nil {
		t.Skip("the script makes its MCP calls with curl")
	}
	home := t.TempDir()
	testenv.SetHome(t, home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	config := filepath.Join(home, ".claude")
	t.Setenv("CLAUDE_CONFIG_DIR", config)
	os.MkdirAll(config, 0o755)
	os.WriteFile(filepath.Join(config, ".credentials.json"), mustJSON(map[string]any{"claudeAiOauth": map[string]any{
		"accessToken": "tok-lead", "refreshToken": "r-lead", "expiresAt": time.Now().Add(24 * time.Hour).UnixMilli(), "subscriptionType": "max"}}), 0o600)
	os.WriteFile(filepath.Join(home, ".claude.json"), mustJSON(map[string]any{
		"oauthAccount": map[string]any{"emailAddress": "lead@example.com", "accountUuid": "u-lead"}}), 0o600)
	dir, logs := t.TempDir(), t.TempDir()
	testenv.Program(t, filepath.Join(dir, "claude"), `#!/bin/sh
if [ "$1" = auth ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"lead@example.com","subscriptionType":"max"}'; exit 0; fi
log=`+logs+`/$$
echo "args $*" > $log
helper=$(printf '%s\n' "$@" | grep -o '"claude-mcp-helper","[^"]*","[^"]*"' | head -1)
cb=$(cat "$(printf '%s' "$helper" | sed 's/^"claude-mcp-helper","//; s/",".*//')")
cp "$(printf '%s' "$helper" | sed 's/.*","//; s/"$//')" `+logs+`/$$.tools
sid=s$$; prev=
for a in "$@"; do
  if [ "$prev" = --resume ]; then sid=$a; fi
  prev=$a
done
proj="$CLAUDE_CONFIG_DIR/projects/$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')"
mkdir -p "$proj"
f="$proj/$sid.jsonl"
had=$(cat "$f" 2>/dev/null | wc -l | tr -d ' ')
echo '{"type":"system","subtype":"init","session_id":"'$sid'"}'
n=0; m=0
say() {
  m=$((m+1))
  echo '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m'$$'-'$m'","model":"claude-opus-5-5","usage":{"input_tokens":1}}}}'
  echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"'"$1"'"}}}'
  echo '{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}}'
  echo '{"type":"stream_event","event":{"type":"message_stop"}}'
  echo '{"type":"assistant","uuid":"a'$$'-'$m'","sessionId":"'$sid'","message":{"role":"assistant","content":[{"type":"text","text":"'"$1"'"}]}}' >> "$f"
  echo '{"type":"assistant","uuid":"a'$$'-'$m'","message":{"id":"m'$$'-'$m'","model":"claude-opus-5-5","content":[{"type":"text","text":"'"$1"'"}]}}'
  echo '{"type":"result","subtype":"success","is_error":false,"result":""}'
}
while read -r line; do
  case "$line" in *'"type":"control_request"'*)
    id=$(printf '%s' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
    echo '{"type":"control_response","response":{"subtype":"success","request_id":"'$id'"}}'
    continue ;;
  esac
  n=$((n+1))
  echo "told $line" >> $log
  echo '{"type":"user","uuid":"u'$$'-'$n'","sessionId":"'$sid'","message":{"role":"user","content":"x"}}' >> "$f"
  case "$line" in *CALLTOOL*)
    m=$((m+1))
    echo '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m'$$'-'$m'","model":"claude-opus-5-5","usage":{"input_tokens":1}}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_R'$$'_'$n'","name":"mcp__magpie__read"}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
    echo '{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}}'
    echo '{"type":"stream_event","event":{"type":"message_stop"}}'
    echo '{"type":"assistant","uuid":"a'$$'-'$m'","sessionId":"'$sid'","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_R'$$'_'$n'","name":"mcp__magpie__read","input":{}}]}}' >> "$f"
    echo '{"type":"assistant","uuid":"a'$$'-'$m'","message":{"id":"m'$$'-'$m'","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"toolu_R'$$'_'$n'","name":"mcp__magpie__read","input":{}}]}}'
    got=$(curl --noproxy '*' -s -m 20 -X POST -H 'Content-Type: application/json' --data '{"tool_call_id":"toolu_R'$$'_'$n'","name":"read","arguments":{}}' "$cb")
    case "$got" in *"No tool can be used"*) got=refused ;; *) got=answered ;; esac
    say "pid $$ had $had turn $n call $got"
    continue ;;
  esac
  say "pid $$ had $had turn $n"
done
`)
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	provider.ForgetAccounts()
	t.Cleanup(provider.ForgetAccounts)
	if err := provider.SaveGroup(provider.Group{ID: "orchestrator", Name: "Orchestrator", Members: []string{"claude/claude-opus-5-5"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	srv := httptest.NewServer(s.Handler())
	t.Cleanup(srv.Close)
	t.Cleanup(s.subscription.abortAll) // before srv.Close: a run's MCP call holds a request open
	t.Setenv("MAGPIE_ADDR", srv.Listener.Addr().String())
	return &prefixHarness{t: t, s: s, srv: srv, logs: logs, config: config}
}

// prefixTools is the client's tools, as the harness's requests offer them.
const prefixTools = `[{"name":"read","description":"Read a file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}},{"name":"bash","description":"Run a command","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}}]`

var prefixAnswer = regexp.MustCompile(`pid (\d+) had (\d+) turn (\d+)( call [a-z]+)?`)

// ask posts messages with extra fields (`"tools":…,` and the like) and
// gives the answer, "pid P had H turn T", and the whole body.
func (h *prefixHarness) ask(extra, messages string) (string, string) {
	h.t.Helper()
	res, err := http.Post(h.srv.URL+"/v1/messages", "application/json", strings.NewReader(
		`{"model":"group/orchestrator","max_tokens":1000,"stream":true,"system":"be brief",`+extra+`"messages":`+messages+`}`))
	if err != nil {
		h.t.Fatal(err)
	}
	defer res.Body.Close()
	b, _ := io.ReadAll(res.Body)
	if res.StatusCode != 200 {
		h.t.Fatalf("%d %.600s", res.StatusCode, b)
	}
	return prefixAnswer.FindString(string(b)), string(b)
}

// log is what the run of pid was started with and told.
func (h *prefixHarness) log(pid string) string {
	b, _ := os.ReadFile(filepath.Join(h.logs, pid))
	return string(b)
}

// pids are the runs started so far.
func (h *prefixHarness) pids() []string {
	entries, _ := os.ReadDir(h.logs)
	var pids []string
	for _, e := range entries {
		if !strings.Contains(e.Name(), ".") {
			pids = append(pids, e.Name())
		}
	}
	return pids
}

// tools is the tools the run of pid's MCP helper offered its agent.
func (h *prefixHarness) tools(pid string) []bridgeTool {
	h.t.Helper()
	b, err := os.ReadFile(filepath.Join(h.logs, pid+".tools"))
	if err != nil {
		h.t.Fatal(err)
	}
	var tools []bridgeTool
	if err := json.Unmarshal(b, &tools); err != nil {
		h.t.Fatalf("%v: %s", err, b)
	}
	return tools
}

func answerPID(t *testing.T, answer string) string {
	t.Helper()
	m := prefixAnswer.FindStringSubmatch(answer)
	if m == nil {
		t.Fatalf("answer %q", answer)
	}
	return m[1]
}

// bbfatx on X: a client asks a conversation's title or summary with
// tool_choice none and its tools as ever, so the request shares the
// conversation's cached prefix. The bridge offered such a turn's Claude
// Code no tools at all, and Anthropic caches a prompt as its tools, then
// its system prompt, then its messages: the turn was a prefix the cache
// had never seen, all of it written again, Claude Code's own tools and
// system prompt too (measured with Claude Code 2.1.316 against a stand-in
// that counts as Anthropic's cache does: 0 read and 7595 written, where
// the same turn offered its tools read 15780 of them). It is offered
// every tool the client sent, and may call none: a call it makes is
// answered at once, never shown to the client, and the reply goes on in
// words.
func TestClaudeToolChoiceNoneKeepsTheTools(t *testing.T) {
	h := newPrefixHarness(t)
	conv := `[` + rmsg("user", "fix the bug") + `,` + rmsg("assistant", "done") + `,` + rmsg("user", "Write a title for this conversation. CALLTOOL") + `]`
	answer, body := h.ask(`"tools":`+prefixTools+`,"tool_choice":{"type":"none"},`, conv)
	pids := h.pids()
	if len(pids) != 1 {
		t.Fatalf("runs %v", pids)
	}
	pid := pids[0]
	var names []string
	for _, tool := range h.tools(pid) {
		names = append(names, tool.Name)
	}
	if strings.Join(names, ",") != "read,bash" {
		t.Errorf("a tool_choice none turn's Claude Code was offered %v, want every tool the client sent", names)
	}
	if !strings.HasSuffix(answer, " call refused") {
		t.Errorf("the turn's call was %q, want it refused", answer)
	}
	if strings.Contains(body, `"tool_use"`) || strings.Contains(body, "toolu_R") {
		t.Errorf("the client was shown a call it chose none of: %.800s", body)
	}
	// a turn that may call them hands its call to the client, as ever
	_, body = h.ask(`"tools":`+prefixTools+`,`, `[`+rmsg("user", "read it. CALLTOOL")+`]`)
	if !strings.Contains(body, `"tool_use"`) || !strings.Contains(body, `"name":"read"`) {
		t.Errorf("a turn of tool_choice auto was not shown its call: %.800s", body)
	}
}

// bbfatx on X: a title or summary a client asks of a conversation
// between its turns, with tool_choice none or no tools, went to a run of
// its own, told the conversation whole, and that run, carrying the
// conversation on as retire took it, let go of the run the conversation
// waited in. The conversation's next turn found no run and a new Claude
// Code was told the whole conversation again, all of it written to the
// cache once more. The run that waits for the next turn is left
// waiting, and the next turn goes on in it.
func TestClaudeAsideLeavesTheConversationsRun(t *testing.T) {
	for _, c := range []struct{ name, extra string }{
		{"tool_choice none", `"tools":` + prefixTools + `,"tool_choice":{"type":"none"},`},
		{"no tools", ``},
	} {
		t.Run(c.name, func(t *testing.T) {
			h := newPrefixHarness(t)
			tools := `"tools":` + prefixTools + `,`
			first, _ := h.ask(tools, `[`+rmsg("user", "fix the bug")+`]`)
			pid := answerPID(t, first)
			conv := rmsg("user", "fix the bug") + `,` + rmsg("assistant", first)
			if got, _ := h.ask(c.extra, `[`+conv+`,`+rmsg("user", "Write a short title for this conversation.")+`]`); answerPID(t, got) == pid {
				t.Fatalf("the title went on in the conversation's run: %q", got)
			}
			next, _ := h.ask(tools, `[`+conv+`,`+rmsg("user", "and the tests")+`]`)
			if answerPID(t, next) != pid || !strings.HasSuffix(next, " turn 2") {
				t.Fatalf("the turn after the title was %q, want it in the conversation's run (pid %s), its second turn", next, pid)
			}
		})
	}
}

// A branch's session is the run's lines as far as the one it is cut
// after, each its own session's; a line still being written is not there
// yet, and one never written is an error, not a branch of the whole.
func TestClaudeCutSessionCopiesAsFarAsTheLine(t *testing.T) {
	was := cutWait
	cutWait = 50 * time.Millisecond
	t.Cleanup(func() { cutWait = was })
	dirs := []string{t.TempDir(), t.TempDir()}
	lines := `{"type":"user","uuid":"a","sessionId":"S"}` + "\n" +
		`{"type":"assistant","parentUuid":"b","uuid":"b","sessionId":"S","message":{"content":"\"uuid\":\"c\""}}` + "\n" +
		`{"type":"user","parentUuid":"b","uuid":"c","sessionId":"S"}` + "\n" +
		`{"type":"assistant","uuid":"d","sessionId":"S"`
	if err := os.WriteFile(filepath.Join(dirs[1], "S.jsonl"), []byte(lines), 0o600); err != nil {
		t.Fatal(err)
	}
	id, err := cutSession(dirs, "S", "b")
	if err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(filepath.Join(dirs[1], id+".jsonl"))
	want := strings.ReplaceAll(strings.Join(strings.SplitAfter(lines, "\n")[:2], ""), `"sessionId":"S"`, `"sessionId":"`+id+`"`)
	if id == "S" || string(got) != want {
		t.Fatalf("copy %s:\n%s\nwant\n%s", id, got, want)
	}
	if src, _ := os.ReadFile(filepath.Join(dirs[1], "S.jsonl")); string(src) != lines {
		t.Fatalf("the run's session changed: %s", src)
	}
	for _, at := range []string{"d", "z"} {
		if id, err := cutSession(dirs, "S", at); err == nil {
			t.Errorf("cut at %s: %s, want an error", at, id)
		}
	}
}

// interactive makes the harness's runs Claude Code's interactive mode:
// its helper is a stand-in that runs the stand-in Claude Code as asked.
func (h *prefixHarness) interactive() {
	h.t.Helper()
	claudeTUIRuns = true
	h.t.Cleanup(func() { claudeTUIRuns = false })
	helper := filepath.Join(h.t.TempDir(), "magpie")
	testenv.Program(h.t, helper, `#!/bin/sh
[ "$1" = "`+claudetui.Command+`" ] || exit 9
shift; claude=$1; shift
exec "$claude" "$@"
`)
	claudeTUIHelper = helper
	h.t.Cleanup(func() { claudeTUIHelper = "" })
}

// Claude Code asks its prompt suggestion after each turn as the
// conversation and one message more, and its run goes on with it (#1638).
// A -p run is taken back from it when the conversation's next turn comes;
// the interactive mode can't be, and was let go, so a new Claude Code was
// told the whole conversation every turn, all of it written to the cache
// again (bbfatx on X). The turn goes on from a copy of the run's session
// cut before the suggestion, in the interactive mode, told only the turn.
func TestClaudeInteractiveSideQuestionBranchesBeforeIt(t *testing.T) {
	h := newPrefixHarness(t)
	h.interactive()
	tools := `"tools":` + prefixTools + `,`
	first, _ := h.ask(tools, `[`+rmsg("user", "fix the bug")+`]`)
	pid := answerPID(t, first)
	conv := rmsg("user", "fix the bug") + `,` + rmsg("assistant", first) + `,` + rmsg("user", "and the tests")
	second, _ := h.ask(tools, `[`+conv+`]`)
	if second != "pid "+pid+" had 0 turn 2" {
		t.Fatalf("second: %q", second)
	}
	conv += `,` + rmsg("assistant", second)
	if got, _ := h.ask(tools, `[`+conv+`,`+rmsg("user", suggestionMode)+`]`); got != "pid "+pid+" had 0 turn 3" {
		t.Fatalf("the suggestion: %q", got)
	}
	third, _ := h.ask(tools, `[`+conv+`,`+rmsg("user", "now commit it")+`]`)
	branch := answerPID(t, third)
	// its session's two turns, the suggestion's left out
	if branch == pid || third != "pid "+branch+" had 4 turn 1" {
		t.Fatalf("the turn after the suggestion: %q (the conversation's run was %s)\n%s", third, pid, h.log(branch))
	}
	log := h.log(branch)
	if told := log[strings.Index(log, "\ntold "):]; strings.Contains(told, "fix the bug") || strings.Contains(told, "SUGGESTION") || !strings.Contains(told, "now commit it") {
		t.Fatalf("the branch was told %.600s, want the new turn only", told)
	}
	// and the conversation goes on in it
	conv += `,` + rmsg("user", "now commit it") + `,` + rmsg("assistant", third)
	if got, _ := h.ask(tools, `[`+conv+`,`+rmsg("user", "push it")+`]`); got != "pid "+branch+" had 4 turn 2" {
		t.Fatalf("the turn after: %q", got)
	}
}
