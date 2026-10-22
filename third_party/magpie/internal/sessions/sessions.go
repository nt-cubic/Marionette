// Package sessions lists the agents' recent sessions from their own session
// files — Claude Code's projects/*/<id>.jsonl (and Qoder's, the same kind),
// Codex's rollout files, OpenCode's database (or its older JSON files) and
// ZCode's, Pi's session files and omp's, DeepSeek Harness's, Cline's, Grok
// Build's, WorkBuddy's and CodeBuddy Code's, Droid's, Cursor CLI's and Alma's chat stores — with the tokens each spent, what that cost at the
// effective price, and the command that resumes it. It only ever reads the
// agents' folders.
//
// The files grow long (hundreds of MB), so each one's parse is kept by path,
// size and time, and a file that only grew is read on from where it was left.
// The parses are kept on disk too, in magpie's cache folder.
package sessions

import (
	"bufio"
	"bytes"
	"cmp"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/klauspost/compress/zstd"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/dirent"
	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// Tokens is a count of tokens. Input excludes what was read from cache.
type Tokens struct {
	Input      int `json:"input"`
	Output     int `json:"output"`
	CacheRead  int `json:"cache_read"`
	CacheWrite int `json:"cache_write"`
	// CacheWrite1h is how many of CacheWrite were written to be kept for an
	// hour, where the agent says (Claude Code's cache_creation)
	CacheWrite1h int `json:"cache_write_1h,omitempty"`
}

func (t *Tokens) add(u Tokens) {
	t.Input += u.Input
	t.Output += u.Output
	t.CacheRead += u.CacheRead
	t.CacheWrite += u.CacheWrite
	t.CacheWrite1h += u.CacheWrite1h
}

func (t *Tokens) sub(u Tokens) {
	t.Input -= u.Input
	t.Output -= u.Output
	t.CacheRead -= u.CacheRead
	t.CacheWrite -= u.CacheWrite
	t.CacheWrite1h -= u.CacheWrite1h
}

// ccCacheCreation is Claude Code's cache writes split by how long they are
// kept.
type ccCacheCreation struct {
	Ephemeral5m int `json:"ephemeral_5m_input_tokens"`
	Ephemeral1h int `json:"ephemeral_1h_input_tokens"`
}

// ccTokens is a Claude Code message's usage, its 1-hour cache writes apart.
func ccTokens(in, out, read, write int, c *ccCacheCreation) Tokens {
	t := Tokens{Input: in, Output: out, CacheRead: read, CacheWrite: write}
	if c != nil {
		t.CacheWrite = max(t.CacheWrite, c.Ephemeral5m+c.Ephemeral1h)
		t.CacheWrite1h = c.Ephemeral1h
	}
	return t
}

func (t Tokens) zero() bool { return t == Tokens{} }

// Model is one model's share of a session.
type Model struct {
	Model string `json:"model"`
	Tokens
	Cost   float64 `json:"cost"`
	Priced bool    `json:"priced"`
}

// ModelEffort is one model and reasoning effort's share of a session.
type ModelEffort struct {
	Model  string `json:"model"`
	Effort string `json:"effort"`
	Tokens
	Reasoning int     `json:"reasoning"`
	Cost      float64 `json:"cost"`
	Priced    bool    `json:"priced"`
}

// CodexIdentity is a Codex thread's confirmed role and parent chain.
type CodexIdentity struct {
	ID            string `json:"id"`
	Role          string `json:"role"`
	ParentSession string `json:"parent_session,omitempty"`
	RootSession   string `json:"root_session,omitempty"`
}

// Session is one agent session.
type Session struct {
	ReadOnly bool `json:"read_only,omitempty"`
	// Gateway: seen only through magpie's gateway, with no file of the
	// agent's on this computer (external.go)
	Gateway       bool          `json:"gateway,omitempty"`
	Agent         string        `json:"agent"` // magpie agent id: claude, codex, opencode, pi, omp, zcode, dsh, cline, qoder, qoder-cn, grok, workbuddy, codebuddy, droid, cursor, hermes, alma, kimi
	ID            string        `json:"id"`
	Cwd           string        `json:"cwd"`
	Title         string        `json:"title"` // the name it was given, else the agent's own title, else the first prompt
	Nickname      string        `json:"nickname,omitempty"`
	Start         time.Time     `json:"start"`
	Last          time.Time     `json:"last"`
	Role          string        `json:"role,omitempty"`
	ParentSession string        `json:"parent_session,omitempty"`
	RootSession   string        `json:"root_session,omitempty"`
	ModelEffort   []ModelEffort `json:"model_effort,omitempty"`
	Models        []Model       `json:"models"`
	Tokens
	Cost     float64 `json:"cost"`     // USD at the effective price, for the priced models
	Unpriced int     `json:"unpriced"` // models that spent tokens but have no known price
	Resume   string  `json:"resume"`   // the command that picks the session up again
	Path     string  `json:"path"`     // its (main) file
	// Carry are the other agents that can carry the session on, reading its
	// file as it is (carry.go)
	Carry []Carry `json:"carry,omitempty"`
	// Transcript says its conversation can be read (TranscriptOf)
	Transcript bool `json:"transcript,omitempty"`
	// Subagents are the conversations its agent started inside it, each
	// read with SubagentTranscript (subagents.go)
	Subagents []Subagent `json:"subagents,omitempty"`
	// WSL is the WSL distro the session ran in, its files read through
	// \\wsl.localhost (see wsl.go); "" for this computer's own
	WSL string `json:"wsl,omitempty"`
}

// PriceOf is the effective price of a model as a session names it, at the
// settings given. Tests swap it; the settings are the ones the listing read,
// so a swapped one prices against the same copy as the default does.
var PriceOf = priceOf

// Limit is how many sessions, the latest by last activity, List reads when
// the caller asks for no particular number. traceRecentFiles sizes the
// window gateway attribution is judged over by it too, so it is not raised
// to make a listing longer: a caller that wants more asks for more.
const Limit = 200

// All is what a listing that means every session asks List for: the count a
// page or a command shows beside the list is taken from every session, so
// the list has to be as long as the count, and this is long past any history
// a computer has. Limit stays what it is because traceRecentFiles sizes
// gateway attribution by it.
const All = 5000

// state is what one file's parse has come to, enough to read on from Off.
type state struct {
	Codex                 *codexUsageState  `json:"-"`
	Claude                *claudeUsageState `json:"-"`
	DBRevision            string            `json:"db_revision,omitempty"`
	Head                  string            `json:"head,omitempty"`
	HeadSize              int               `json:"head_size,omitempty"`
	ContentHash           string            `json:"content_hash,omitempty"`
	Size                  int64             `json:"size"`
	Mod                   int64             `json:"mod"` // unix nanoseconds
	Off                   int64             `json:"off"` // after the last whole line read
	ID                    string            `json:"id,omitempty"`
	CodexIdentity         *CodexIdentity    `json:"codex_identity,omitempty"`
	CodexIdentityConflict bool              `json:"codex_identity_conflict,omitempty"`
	CodexNickname         string            `json:"codex_nickname,omitempty"`
	CodexSpawnNames       map[string]string `json:"codex_spawn_names,omitempty"`
	CodexSpawnConflicts   map[string]bool   `json:"codex_spawn_conflicts,omitempty"`
	CodexAgentIDs         map[string]string `json:"codex_agent_ids,omitempty"`
	CodexAgentConflicts   map[string]bool   `json:"codex_agent_conflicts,omitempty"`
	Cwd                   string            `json:"cwd,omitempty"`
	Title                 string            `json:"title,omitempty"`
	Named                 string            `json:"named,omitempty"`  // the agent's own title for it
	Custom                string            `json:"custom,omitempty"` // the name the user gave it
	First                 string            `json:"first,omitempty"`  // the first message, when no prompt looked typed
	Start                 time.Time         `json:"start"`
	Last                  time.Time         `json:"last"`
	Models                map[string]Tokens `json:"models,omitempty"`
	// Days is Models again, split by the local date each message was
	// written on: a session that runs past midnight counts on both days.
	Days map[string]*day `json:"days,omitempty"`
	// Claude Code: the message last counted, whose later lines repeat it,
	// and the day it was counted on
	Msg      string `json:"msg,omitempty"`
	MsgModel string `json:"msg_model,omitempty"`
	MsgUse   Tokens `json:"msg_use"`
	MsgDay   string `json:"msg_day,omitempty"`
	// Codex: the model in use, and its running total (input with cache) last seen
	Model string  `json:"model,omitempty"`
	Total *Tokens `json:"total,omitempty"`
	// Codex: the model_provider its session_meta names (codex_provider.go)
	Provider string `json:"provider,omitempty"`
	// Pi: in a forked session, the time it was forked; the lines before
	// it are the copy of the session it was forked from
	Since time.Time `json:"since,omitzero"`
	// Grok Build: where its updates were read to, and what the read
	// carries on with
	Grok *grokCarry `json:"grok,omitempty"`
	// Droid: where its transcript was read to, and what the read carries
	// on with
	Droid *droidCarry `json:"droid,omitempty"`
	// Kimi Code: where its wire file was read to, and what the read
	// carries on with
	Kimi *kimiCarry `json:"kimi,omitempty"`
	// dsh: where its file was read to, and its header
	Dsh *dshCarry `json:"dsh,omitempty"`
	// Cline: the messages file its counts were read from
	Cline *clineCarry `json:"cline,omitempty"`
}

// day is one local date's share of a file: tokens by model, and the time
// the session was at work on it.
type day struct {
	Models map[string]Tokens `json:"m,omitempty"`
	// Active is milliseconds of work: the gaps between one line and the
	// next, each counted when shorter than idleGap and put on the later
	// line's day. A longer pause (lunch, the night, a session picked up
	// again next week) counts nothing. A subagent's file counts none, as
	// it runs while its session's own file goes on.
	Active int64 `json:"a,omitempty"`
	// Hours is Active again, by the local hour of the day it was put on
	// (24 of them, or none)
	Hours []int64 `json:"h,omitempty"`
	// Prompts are the messages typed in the session's own file, Replies
	// the agent's messages back (Claude Code's, Codex's, Pi's and omp's)
	Prompts int `json:"u,omitempty"`
	Replies int `json:"r,omitempty"`
	// Tools are the tool calls made, by the tool's name, and Skills the
	// skills called up, by the skill's (subagents' files included)
	Tools  map[string]int `json:"t,omitempty"`
	Skills map[string]int `json:"k,omitempty"`
}

// tool counts a call of a tool, and of a skill when it calls one up; a
// skill called up with no tool (Pi's /skill:name) has no name.
func (s *state) tool(at time.Time, name, skill string) {
	if name == "" && skill == "" {
		return
	}
	d := s.day(dateOf(at))
	if name != "" {
		if d.Tools == nil {
			d.Tools = map[string]int{}
		}
		d.Tools[name]++
	}
	if skill != "" {
		if d.Skills == nil {
			d.Skills = map[string]int{}
		}
		d.Skills[skill]++
	}
}

// idleGap is the longest pause between two lines still counted as work.
const idleGap = 5 * time.Minute

// dateOf is the local date of a time, "" for none.
func dateOf(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.In(time.Local).Format(time.DateOnly)
}

func (s *state) day(date string) *day {
	if s.Days == nil {
		s.Days = map[string]*day{}
	}
	d := s.Days[date]
	if d == nil {
		d = &day{}
		s.Days[date] = d
	}
	return d
}

// use counts a model's tokens, spent on a date.
func (s *state) use(date, model string, t Tokens) {
	if t.zero() {
		return
	}
	if s.Models == nil {
		s.Models = map[string]Tokens{}
	}
	m := s.Models[model]
	m.add(t)
	s.Models[model] = m
	d := s.day(date)
	if d.Models == nil {
		d.Models = map[string]Tokens{}
	}
	m = d.Models[model]
	m.add(t)
	d.Models[model] = m
}

// unuse takes back what use counted.
func (s *state) unuse(date, model string, t Tokens) {
	if m, ok := s.Models[model]; ok {
		m.sub(t)
		s.Models[model] = m
	}
	if d := s.Days[date]; d != nil {
		if m, ok := d.Models[model]; ok {
			m.sub(t)
			d.Models[model] = m
		}
	}
}

// saw notes the time of a line and, in a session's own file, the work
// since the line before.
func (s *state) saw(t time.Time, main bool) {
	if t.IsZero() {
		return
	}
	if s.Start.IsZero() || t.Before(s.Start) {
		s.Start = t
	}
	if t.After(s.Last) {
		if gap := t.Sub(s.Last); main && !s.Last.IsZero() && gap < idleGap {
			d := s.day(dateOf(t))
			d.Active += gap.Milliseconds()
			if d.Hours == nil {
				d.Hours = make([]int64, 24)
			}
			d.Hours[t.In(time.Local).Hour()] += gap.Milliseconds()
		}
		s.Last = t
	}
}

func (s *state) clone() *state {
	c := *s
	c.Codex = s.Codex.clone()
	c.Claude = s.Claude.clone()
	c.Models = make(map[string]Tokens, len(s.Models))
	for k, v := range s.Models {
		c.Models[k] = v
	}
	if s.Total != nil {
		t := *s.Total
		c.Total = &t
	}
	c.CodexSpawnNames = maps.Clone(s.CodexSpawnNames)
	c.CodexSpawnConflicts = maps.Clone(s.CodexSpawnConflicts)
	c.CodexAgentIDs = maps.Clone(s.CodexAgentIDs)
	c.CodexAgentConflicts = maps.Clone(s.CodexAgentConflicts)
	c.Days = make(map[string]*day, len(s.Days))
	for k, v := range s.Days {
		d := &day{Active: v.Active, Models: make(map[string]Tokens, len(v.Models)), Prompts: v.Prompts, Replies: v.Replies}
		if v.Hours != nil {
			d.Hours = append([]int64(nil), v.Hours...)
		}
		if v.Tools != nil {
			d.Tools = maps.Clone(v.Tools)
		}
		if v.Skills != nil {
			d.Skills = maps.Clone(v.Skills)
		}
		for m, t := range v.Models {
			d.Models[m] = t
		}
		c.Days[k] = d
	}
	return &c
}

// file is one session file found on disk.
type file struct {
	agent string
	key   string // agent:session id — a session may span files
	path  string
	main  bool // the session's own file, not a subagent's
	size  int64
	mod   time.Time
	// OpenCode and ZCode: the session, and where it is kept; parent is
	// the session that started it, "" for one the user did
	sid    string
	parent string
	oc     ocStore
	// Cline: the session's manifest, beside its messages; Grok Build: its
	// summary.json, beside its updates; Droid: its settings, beside its
	// transcript
	manifest string
	hermes   *hermesDB
	// Alma: its database, and what tells the chat's rows changed
	alma     *almaStore
	rev      string
	readOnly bool
	editor   *cursorEditorMeta
	// wsl: the WSL distro whose home it is in; cold: that distro isn't
	// running, and the file, as last listed, is not to be opened (that
	// would start the distro)
	wsl  string
	cold bool
}

// ClaudeDir is Claude Code's folder: $CLAUDE_CONFIG_DIR, else ~/.claude.
func ClaudeDir() string { return appdir.ClaudeDir() }

// CodexDir is Codex's folder: $CODEX_HOME, else ~/.codex.
func CodexDir() string { return appdir.CodexHome() }

func stat(f *file) bool {
	fi, err := os.Stat(f.path)
	if err != nil || !fi.Mode().IsRegular() {
		return false
	}
	f.size, f.mod = fi.Size(), fi.ModTime()
	return true
}

// ccFiles are the session files of an agent that keeps them as Claude Code
// does, under its folder's projects/: a session's own <id>.jsonl in its
// project's folder, its subagents' in <id>/subagents/, and the agents of
// its workflows (ultracode) in <id>/subagents/workflows/<run>/agent-*.jsonl
// — beside each run's journal.jsonl, which isn't a transcript.
func ccFiles(agent, dir string) []file {
	projects := filepath.Join(dir, "projects")
	var out []file
	for _, project := range readDirectory(projects) {
		if !dirent.IsFolder(projects, project) {
			continue
		}
		root := filepath.Join(projects, project.Name())
		for _, e := range readDirectory(root) {
			path := filepath.Join(root, e.Name())
			if !e.IsDir() && strings.HasSuffix(e.Name(), ".jsonl") {
				f := file{agent: agent, key: agent + ":" + strings.TrimSuffix(e.Name(), ".jsonl"), path: path, main: true}
				if stat(&f) {
					out = append(out, f)
				}
			} else if dirent.IsFolder(root, e) {
				subs := filepath.Join(path, "subagents")
				for _, sub := range readDirectory(subs) {
					if sub.IsDir() || !strings.HasSuffix(sub.Name(), ".jsonl") {
						continue
					}
					f := file{agent: agent, key: agent + ":" + e.Name(), path: filepath.Join(subs, sub.Name())}
					if stat(&f) {
						out = append(out, f)
					}
				}
				for _, run := range readDirectory(filepath.Join(subs, "workflows")) {
					if !dirent.IsFolder(filepath.Join(subs, "workflows"), run) {
						continue
					}
					dir := filepath.Join(subs, "workflows", run.Name())
					for _, sub := range readDirectory(dir) {
						if sub.IsDir() || !strings.HasPrefix(sub.Name(), "agent-") || !strings.HasSuffix(sub.Name(), ".jsonl") {
							continue
						}
						f := file{agent: agent, key: agent + ":" + e.Name(), path: filepath.Join(dir, sub.Name())}
						if stat(&f) {
							out = append(out, f)
						}
					}
				}
			}
		}
	}
	return out
}

// allFiles are every agent's session files.
// filesHook, when a test sets it, runs on each pass over every agent's
// files.
var filesHook func()

func allFiles() []file {
	if filesHook != nil {
		filesHook()
	}
	var out []file
	for _, fs := range [][]file{callFiles(), openCodeFiles(), piFiles(),
		wslFiles("pi", "omp", "kimi"), zcodeFiles(), dshFiles(), clineFiles(), ccFiles("qoder", QoderDir("qoder")), ccFiles("qoder-cn", QoderDir("qoder-cn")),
		grokFiles(), workbuddyFiles(), codebuddyFiles(), droidFiles(), ompFiles(), cursorFiles(), hermesFiles(), almaFiles(), kimiFiles()} {
		out = append(out, fs...)
	}
	return out
}

// Dirs are the folders the sessions are read from: Claude Code's and
// Codex's, and the other agents' where they keep sessions on this computer.
func Dirs() []string {
	out := []string{ClaudeDir(), CodexDir()}
	for _, d := range []struct{ dir, sessions string }{
		{OpenCodeDir(), OpenCodeDir()},
		{PiDir(), PiDir()},
		{ZCodeDir(), zcodeDB()},
		{DshDir(), filepath.Join(DshDir(), "sessions")},
		{ClineSessionDir(), ClineSessionDir()},
		{QoderDir("qoder"), filepath.Join(QoderDir("qoder"), "projects")},
		{QoderDir("qoder-cn"), filepath.Join(QoderDir("qoder-cn"), "projects")},
		{GrokDir(), filepath.Join(GrokDir(), "sessions")},
		{WorkBuddyDir(), filepath.Join(WorkBuddyDir(), "projects")},
		{CodeBuddyDir(), filepath.Join(CodeBuddyDir(), "projects")},
		{OmpDir(), filepath.Join(OmpDir(), "sessions")},
		{FactoryDir(), filepath.Join(FactoryDir(), "sessions")},
		{CursorDir(), filepath.Join(CursorDir(), "chats")},
		{cursorDataDir(), filepath.Join(cursorDataDir(), "projects")},
		{filepath.Dir(cursorEditorDB()), cursorEditorDB()},
		{AlmaDir(), almaDB()},
		{MiniMaxDir(), minimaxSessions()},
		{KimiCodeDir(), kimiSessions()},
	} {
		if _, err := os.Stat(d.sessions); err == nil {
			out = append(out, d.dir)
		}
	}
	out = append(out, HermesDirs()...)
	out = append(out, wslDirs()...)
	return out
}

// rollout-2026-09-20T15-48-28-<thread id>[_<segment>].jsonl, and .jsonl.zst
// once the Codex app's "compress local chat history" has packed an older one
var rolloutName = regexp.MustCompile(`^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-([0-9A-Za-z-]+?)(?:_[0-9A-Za-z-]+)?\.jsonl(?:\.zst)?$`)

// zstSuffix ends a rollout the Codex app has compressed.
const zstSuffix = ".zst"

func codexFiles() []file { return codexFilesIn(CodexDir()) }

// walkLinked is filepath.WalkDir over root, which can be a link to the
// folder: the Codex CLI keeps .codex on another drive by making
// ~/.codex/sessions a junction to it, and a symlink does the same
// elsewhere. WalkDir lstat()s the root it is handed, where a link is no
// folder at all, so it walked nothing behind it (#2233). It walks where
// the link leads (proc.RealPath, since filepath.EvalSymlinks leaves a
// junction as it is) and hands fn each path under root, not under the
// link's target: an agent's folder is read back out of a session's path
// (Codex's config.toml and state databases), and the target holds none
// of it. A root that is no link is walked as it is.
func walkLinked(root string, fn fs.WalkDirFunc) error {
	walk := root
	if fi, err := os.Lstat(root); err == nil && !fi.IsDir() {
		if real, err := proc.RealPath(root); err == nil {
			walk = real
		}
	}
	return filepath.WalkDir(walk, func(p string, d fs.DirEntry, err error) error {
		if walk != root {
			if rel, err := filepath.Rel(walk, p); err == nil {
				p = filepath.Join(root, rel)
			}
		}
		return fn(p, d, err)
	})
}

// codexFilesIn are the rollouts in a Codex folder.
func codexFilesIn(dir string) []file {
	var out []file
	for _, folder := range []string{"sessions", "archived_sessions"} {
		walkLinked(filepath.Join(dir, folder), func(p string, d os.DirEntry, err error) error {
			if err != nil || d.IsDir() {
				return nil
			}
			m := rolloutName.FindStringSubmatch(d.Name())
			if m == nil {
				return nil
			}
			fi, err := d.Info()
			if err != nil || !fi.Mode().IsRegular() {
				return nil
			}
			out = append(out, file{agent: "codex", key: "codex:" + m[1], path: p, main: true, size: fi.Size(), mod: fi.ModTime()})
			return nil
		})
	}
	// During compression, prefer the readable plain twin in the same
	// directory: the new compressed file may not have its trailer yet.
	plain := map[string]bool{}
	for _, f := range out {
		if !strings.HasSuffix(f.path, zstSuffix) {
			plain[f.path] = true
		}
	}
	kept := out[:0]
	for _, f := range out {
		if q, ok := strings.CutSuffix(f.path, zstSuffix); !ok || !plain[q] {
			kept = append(kept, f)
		}
	}
	return distinctCodexFiles(kept)
}

// rolloutTwin is a Codex rollout's file in its other form: the compressed
// one of a plain one, the plain one of a compressed one.
func rolloutTwin(p string) string {
	if q, ok := strings.CutSuffix(p, zstSuffix); ok {
		return q
	}
	return p + zstSuffix
}

var (
	// storeMu keeps the reads of the sessions apart from the changes made
	// to their files (Delete, a provider move, a project relocation): a
	// read takes it shared, so reads never wait on one another, and a
	// change alone. The store handles they share live as long as their last
	// user (useDBs), not as long as the read that opened them.
	storeMu sync.RWMutex
	mu      sync.Mutex
	cache   map[string]*state // path → parse
	// parsing holds the files a refresh is reading now, each closed when
	// its parse is in the cache: another reader serves the parse kept of
	// such a file rather than reading it a second time, and waits only for
	// one it has no parse of yet (#2776: the start's warm-up held every
	// page for its whole read)
	parsing         = map[string]chan struct{}{}
	loaded          bool
	cacheGeneration uint64
	// the zone the parses in memory date their days in
	cacheZone string
)

// CachePath is where the parses are kept between runs.
func CachePath() string { return filepath.Join(filepath.Dir(catalog.CachePath()), "sessions.json") }

// cacheVersion changes when a parse would come out differently, so the
// parses kept by an older magpie are read again.
// 2: each file's usage by day, and its active time
// 3: the active time by hour of the day
// 4: the tool calls and skills a day
// 5: again, for the prompts and replies a day, which an early 4 left out
// 6: per-call metadata and continuation share the session file scan
// 7: Codex's recorded provider and creator identity
// 8: keep only summaries here; request metadata has per-file shards.
// 9: validate the previous full prefix before treating growth as an append.
// 10: Pi's and omp's prompts, replies, tool calls and skills.
// 11: Codex's input without what it wrote to the cache (#589).
// 16: count Codex response records and compaction usage.
// 17: reconcile recent Claude message revisions.
// 18: Claude Code's 1-hour cache writes apart from its 5-minute ones.
// 19: the names the user gave Claude Code's and WorkBuddy's (custom-title).
// 20: Codex's source and direct parent thread identity.
// 21: Codex subagent nicknames and parent spawn task names.
const cacheVersion = 21

type cacheFile struct {
	Version int               `json:"version"`
	Zone    string            `json:"zone"` // the time zone its days are dates in
	Files   map[string]*state `json:"files"`
}

// zone tells time zones apart well enough for the days kept by date: their
// offsets in winter and in summer. Moved to another, the files are read again.
func zone() string {
	y := time.Now().Year()
	_, w := time.Date(y, 1, 1, 0, 0, 0, 0, time.Local).Zone()
	_, s := time.Date(y, 7, 1, 0, 0, 0, 0, time.Local).Zone()
	return strconv.Itoa(w) + "/" + strconv.Itoa(s)
}

func loadCache() {
	z := zone()
	if loaded && z == cacheZone {
		return
	}
	loaded, cacheZone = true, z
	cacheGeneration++
	cache = map[string]*state{}
	saved() // the kept file as the last save left it
	var c cacheFile
	if b, err := os.ReadFile(CachePath()); err == nil && json.Unmarshal(b, &c) == nil && c.Version == cacheVersion && c.Zone == z && c.Files != nil {
		cache = c.Files
	}
}

// saveCache keeps the parses on disk. Writing the whole index (tens of MB
// for a long history) takes a second or more, so it is done behind the
// request: the parses in the cache are never changed once put there (a new
// parse starts from a clone), so a copy of the map is a snapshot; saves in a
// row are folded into the latest.
func saveCache() {
	snap := save{cacheFile{Version: cacheVersion, Zone: zone(), Files: maps.Clone(cache)}, CachePath()}
	saving.Lock()
	defer saving.Unlock()
	saving.next = &snap
	if saving.running {
		return
	}
	saving.running = true
	go func() {
		for {
			saving.Lock()
			f := saving.next
			saving.next = nil
			if f == nil {
				saving.running = false
				saving.Broadcast()
				saving.Unlock()
				return
			}
			saving.Unlock()
			writeCache(f)
		}
	}()
}

var saving struct {
	sync.Mutex
	*sync.Cond
	running bool
	next    *save
}

type save struct {
	cacheFile
	path string
}

func init() { saving.Cond = sync.NewCond(&saving.Mutex) }

// Saved waits for the index being saved, so a process about to end leaves
// it on disk.
func Saved() { saved() }

// saved waits for the saves under way.
func saved() {
	saving.Lock()
	for saving.running {
		saving.Wait()
	}
	saving.Unlock()
}

// saveHook, when a test sets it, runs before each save is written.
var saveHook func()

func writeCache(c *save) {
	if saveHook != nil {
		saveHook()
	}
	b, err := json.Marshal(c.cacheFile)
	if err != nil {
		return
	}
	dir := filepath.Dir(c.path)
	if os.MkdirAll(dir, 0o755) != nil {
		return
	}
	tmp, err := os.CreateTemp(dir, "sessions-*.json")
	if err != nil {
		return
	}
	_, err = tmp.Write(b)
	if cerr := tmp.Close(); err == nil {
		err = cerr
	}
	if err != nil || os.Rename(tmp.Name(), c.path) != nil {
		os.Remove(tmp.Name())
	}
}

// refresh parses those of want that changed since their parse was kept, a
// few files at a time, and keeps the parses of every file still on disk
// (all of them), read by List or by Stats. With no all-set it leaves other
// cached agents alone, for a Codex-only identity refresh.
func refresh(want, all []file) {
	defer func() { trimSummaryRevisions(time.Now()) }()
	var todo, waits []file
	for _, f := range want {
		if f.cold {
			continue // in a stopped WSL distro: what was read of it stands
		}
		if s := cache[f.path]; s == nil || s.Size != f.size || s.Mod != f.mod.UnixNano() || (f.agent == "hermes" && (f.hermes == nil || f.rev == "" || s.DBRevision != f.rev)) || ((f.agent == "alma" || f.agent == "cursor") && s.DBRevision != f.rev) {
			if parsing[f.path] != nil {
				// another refresh is reading it: its last parse stands
				// until that one is in, and one never read is waited for
				if s == nil {
					waits = append(waits, f)
				}
				continue
			}
			todo = append(todo, f)
		}
	}
	if len(waits) > 0 {
		defer waitParsing(waits)
	}
	gone := false
	if len(cache) > 0 && all != nil {
		on := make(map[string]bool, len(all))
		for _, f := range all {
			on[f.path] = true
		}
		for p := range cache {
			if !on[p] {
				delete(cache, p)
				gone = true
			}
		}
	}
	trimSummaryRevisions(time.Now())
	if len(todo) == 0 {
		if gone {
			saveCache()
		}
		return
	}
	// Limit cold-scan workers to the windows that can survive publication.
	// Otherwise a batch of historical files could retain every window until
	// the whole scan finishes.
	windows := summaryRevisionWindows(todo, time.Now())
	// the biggest first, so no long file is left to run on alone at the
	// end; and twice the cores, as the reading waits on the disk
	sort.Slice(todo, func(i, j int) bool { return todo[i].size-offOf(todo[i]) > todo[j].size-offOf(todo[j]) })
	left := make(map[string]int64, len(todo))
	var total int64
	for _, f := range todo {
		left[f.path] = f.size - offOf(f)
		total += left[f.path]
	}
	progress.start(len(todo), total)
	type job struct {
		f      file
		old    *state
		parsed *state
	}
	jobs := make([]job, len(todo))
	for i, f := range todo {
		jobs[i] = job{f: f, old: cache[f.path]}
		parsing[f.path] = make(chan struct{})
	}
	generation := cacheGeneration
	mu.Unlock()
	ch := make(chan int)
	finished := make(chan int, len(jobs))
	for i := 0; i < min(max(8, 2*runtime.NumCPU()), 32, len(todo)); i++ {
		go func() {
			for i := range ch {
				j := &jobs[i]
				// its file's calls lock is taken before a read slot, as
				// readCalls takes them
				calls := callsAlong(j.f)
				release := readSlot()
				end := readBegin()
				if parseHook != nil {
					parseHook()
				}
				j.parsed = parseWith(j.f, j.old, calls)
				end()
				release()
				if (j.parsed.Claude != nil && !windows[j.f.path]) || (j.parsed.Claude == nil && !recentRevision(j.parsed.Mod, j.parsed.revisionWeight(), time.Now())) {
					j.parsed = j.parsed.withoutRevisions()
				}
				progress.files.Add(1)
				progress.read.Add(left[j.f.path])
				finished <- i
			}
		}()
	}
	go func() {
		for i := range jobs {
			ch <- i
		}
		close(ch)
	}()
	// publish puts a finished parse in the cache at once, for a reader
	// waiting on it, and lets the file be read again.
	publish := func(i int) {
		j := &jobs[i]
		// A concurrent read may already have published a newer parse.
		if generation == cacheGeneration && cache[j.f.path] == j.old {
			cache[j.f.path] = j.parsed
		}
		close(parsing[j.f.path])
		delete(parsing, j.f.path)
	}
	// What is read is kept on disk every checkpointEvery, not only at the
	// end: a first read of a long history on a slow disk runs for minutes,
	// and a magpie quit before the end (its CPU full, #1665) kept nothing
	// of it, so the next start read it all again.
	tick := time.NewTicker(checkpointEvery)
	defer tick.Stop()
	unsaved := false
	for n := 0; n < len(jobs); {
		select {
		case i := <-finished:
			mu.Lock()
			publish(i)
			mu.Unlock()
			unsaved = true
			n++
		case <-tick.C:
			if !unsaved {
				continue
			}
			mu.Lock()
			if generation == cacheGeneration {
				saveCache()
			}
			mu.Unlock()
			unsaved = false
		}
	}
	mu.Lock()
	progress.finish()
	if generation == cacheGeneration {
		saveCache()
	}
}

// waitParsing waits, with mu let go, for the files other refreshes are
// reading, and reads itself any of them that came to nothing (a Reset
// dropped its parse).
func waitParsing(fs []file) {
	for {
		var chans []chan struct{}
		for _, f := range fs {
			if ch := parsing[f.path]; ch != nil {
				chans = append(chans, ch)
			}
		}
		if len(chans) == 0 {
			break
		}
		mu.Unlock()
		for _, ch := range chans {
			<-ch
		}
		mu.Lock()
	}
	var missing []file
	for _, f := range fs {
		if cache[f.path] == nil {
			missing = append(missing, f)
		}
	}
	if len(missing) > 0 {
		refresh(missing, nil)
	}
}

// checkpointEvery is how often a long read of changed files keeps what it
// has read so far on disk.
var checkpointEvery = 5 * time.Second

// parseHook, when a test sets it, runs before each changed file is parsed.
var parseHook func()

// progress is how far the reading of changed files has got, for the page to
// show while it waits: files and bytes read of those to read.
var progress indexing

type indexing struct {
	guard       sync.Mutex
	active      int
	on          atomic.Bool
	files, read atomic.Int64
	todo, bytes atomic.Int64
}

func (p *indexing) start(n int, total int64) {
	p.guard.Lock()
	defer p.guard.Unlock()
	if p.active == 0 {
		p.files.Store(0)
		p.read.Store(0)
		p.todo.Store(int64(n))
		p.bytes.Store(total)
	} else {
		p.todo.Add(int64(n))
		p.bytes.Add(total)
	}
	p.active++
	p.on.Store(true)
}

func (p *indexing) finish() {
	p.guard.Lock()
	defer p.guard.Unlock()
	p.active--
	p.on.Store(p.active > 0)
}

// Progress is how far the sessions being read have got; Indexing is false
// when none are.
type Progress struct {
	Indexing bool  `json:"indexing"`
	Files    int64 `json:"files"`
	Done     int64 `json:"done"`
	Bytes    int64 `json:"bytes"`
	Read     int64 `json:"read"`
}

// Indexing is the reading's progress.
func Indexing() Progress {
	p := &progress
	return Progress{Indexing: p.on.Load(), Files: p.todo.Load(), Done: p.files.Load(), Bytes: p.bytes.Load(), Read: min(p.read.Load(), p.bytes.Load())}
}

// offOf is where the parse kept of f left off.
func offOf(f file) int64 {
	if s := cache[f.path]; s != nil && s.Off <= f.size {
		return s.Off
	}
	return 0
}

// pricer looks up the price of each model once, out of one read of the
// settings: a listing names the same handful of models over and over, and
// each of them otherwise read the file and parsed it again. Every model is
// priced against the copy read here, so a listing is one snapshot of the
// prices rather than a reading per row, and a price changed while it is
// read takes effect in the next listing rather than halfway through this
// one.
func pricer() func(string) *catalog.Price {
	prices := map[string]*catalog.Price{}
	s := settings.Load()
	return func(model string) *catalog.Price {
		if p, ok := prices[model]; ok {
			return p
		}
		var pp *catalog.Price
		if p, ok := PriceOf(s, model); ok {
			pp = &p
		}
		prices[model] = pp
		return pp
	}
}

// Reset forgets the kept parses, in memory only.
func Reset() {
	desktopTitles.Lock()
	desktopTitles.names, desktopTitles.last = nil, nil
	desktopTitles.Unlock()
	directoryCache.Lock()
	directoryCache.entries = map[string]directoryEntry{}
	directoryCache.Unlock()
	resetCalls()
	droidCallers.reset()
	grokKinds.reset()
	wslReset()
	forgetHermesPrints()
	mu.Lock()
	defer mu.Unlock()
	saved()
	cache, loaded = nil, false
	cacheGeneration++
}

// List reads the latest sessions of every agent, the most recently active
// first, at most limit of them (Limit when 0).
func List(limit int) []Session {
	storeMu.RLock()
	defer storeMu.RUnlock()
	if limit <= 0 {
		limit = Limit
	}
	mu.Lock()
	defer mu.Unlock()
	loadCache()

	defer useDBs()()
	files := allFiles()
	groups := map[string][]file{}
	latest := map[string]time.Time{}
	for _, f := range files {
		groups[f.key] = append(groups[f.key], f)
		if f.mod.After(latest[f.key]) {
			latest[f.key] = f.mod
		}
	}
	keys := make([]string, 0, len(groups))
	for k := range groups {
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool {
		if !latest[keys[i]].Equal(latest[keys[j]]) {
			return latest[keys[i]].After(latest[keys[j]])
		}
		return keys[i] < keys[j]
	})
	// the keys are not truncated yet: assemble below drops an empty
	// session, and a nonempty one must take its place rather than the
	// page going short - an empty file is not a session (#1320)
	// every changed file, not just the latest sessions': the first read
	// indexes them all in one run the page can show, and the stats read
	// after it has nothing left to do
	refresh(files, files)
	identities := codexIdentitySnapshot(files)
	taskTitles := codexSubagentTaskTitles(files)

	price := pricer()
	out := []Session{}
	for _, k := range keys {
		if s, ok := assemble(groups[k], price); ok {
			setSessionCodexIdentity(&s, identities)
			setSessionCodexTaskTitle(&s, taskTitles)
			out = append(out, s)
			if len(out) == limit {
				break
			}
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Last.After(out[j].Last) })
	return out
}

// sortSessionFiles puts a session's own file first; for Codex the earliest
// segment.
func sortSessionFiles(fs []file) {
	sort.SliceStable(fs, func(i, j int) bool {
		if fs[i].main != fs[j].main {
			return fs[i].main
		}
		return filepath.Base(fs[i].path) < filepath.Base(fs[j].path)
	})
}

// sessionID is a session's id: its key's, or for an agent that writes it
// inside the file (idInFile), what its own file's parse read.
func sessionID(fs []file) string {
	sortSessionFiles(fs)
	id := strings.TrimPrefix(fs[0].key, fs[0].agent+":")
	if idInFile(fs[0].agent) && fs[0].main {
		if st := cache[fs[0].path]; st != nil && st.ID != "" {
			id = st.ID
		}
	}
	return id
}

// assemble puts a session together from its files' parses.
func assemble(fs []file, price func(string) *catalog.Price) (Session, bool) {
	s, ok := assembleListed(fs, price)
	if !ok {
		return s, false
	}
	if !s.ReadOnly {
		s.Resume = resumeCommand(s.WSL, s.Agent, s.ID, s.Cwd, s.Path)
	}
	s.Carry, s.Transcript = carries(s), HasTranscript(s.Agent)
	if s.Transcript {
		s.Subagents = subagentsOf(s, fs)
	}
	return s, true
}

// assembleListed is assemble without what only a listed session's row shows (how
// to resume it, its subagents): whether List lists the session, and its
// agent, id, title and usage. The stats ask it of every session on each
// poll, and building every session's subagents there read each Claude Code
// subagent's file again (3.5 GB of a live magpie's 6.3 GB of scans).
func assembleListed(fs []file, price func(string) *catalog.Price) (Session, bool) {
	sortSessionFiles(fs)
	s := Session{Agent: fs[0].agent, Path: fs[0].path, Models: []Model{}, WSL: fs[0].wsl}
	for _, f := range fs {
		s.ReadOnly = s.ReadOnly || f.readOnly
	}
	s.ID = sessionID(fs)
	var custom, named, first string
	models := map[string]*Model{}
	for _, f := range fs {
		st := cache[f.path]
		if st == nil {
			continue
		}
		if f.main {
			if s.Cwd == "" {
				s.Cwd = st.Cwd
			}
			if s.Title == "" {
				s.Title = st.Title
			}
			if st.Named != "" {
				named = st.Named
			}
			if st.Custom != "" {
				custom = st.Custom
			}
			if first == "" {
				first = st.First
			}
		}
		if s.Agent == "codex" && st.CodexNickname != "" {
			s.Nickname = st.CodexNickname
		}
		if !st.Start.IsZero() && (s.Start.IsZero() || st.Start.Before(s.Start)) {
			s.Start = st.Start
		}
		if st.Last.After(s.Last) {
			s.Last = st.Last
		}
		for name, t := range st.Models {
			m := models[name]
			if m == nil {
				m = &Model{Model: name}
				models[name] = m
			}
			m.add(t)
		}
	}
	// the name the session goes by in its agent: one the user gave it, else
	// Codex's thread name (session_index.jsonl), else the title the agent
	// made, else the first prompt, else the first message
	if s.Agent == "codex" && s.WSL == "" && custom == "" {
		custom = CodexTitles([]string{s.ID})[s.ID]
	}
	if s.Agent == "claude-desktop" && s.WSL == "" {
		custom = cmp.Or(DesktopTitles([]string{s.ID})[s.ID], custom)
	}
	s.Title = cmp.Or(custom, named, s.Title, first)
	for _, m := range models {
		if p := price(m.Model); p != nil {
			// a sum of the session's calls of the model, at its base price:
			// which call went over a long-context tier isn't known here
			m.Cost, m.Priced = p.At(0).CostSplit(m.Input, m.Output, m.CacheRead, m.CacheWrite, m.CacheWrite1h), true
			s.Cost += m.Cost
		} else {
			s.Unpriced++
		}
		s.Tokens.add(m.Tokens)
		s.Models = append(s.Models, *m)
	}
	sort.Slice(s.Models, func(i, j int) bool {
		a, b := s.Models[i], s.Models[j]
		if a.Input+a.Output != b.Input+b.Output {
			return a.Input+a.Output > b.Input+b.Output
		}
		return a.Model < b.Model
	})
	if s.Last.IsZero() {
		s.Last = fs[0].mod
	}
	if s.Title == "" && s.Tokens.zero() {
		return s, false // nothing was said in it
	}
	return s, true
}

// parserFor is the source of truth for both parsing and window admission.
// A default line reader is Claude-compatible, including both Qoder variants.
type sessionParser struct {
	whole func(file) *state
	// grow reads a file on from where the kept parse of it left off, when
	// the file has only grown since (tail.go), else whole
	grow   func(file, *state) *state
	line   func(*state, []byte, bool)
	claude bool
}

func parserFor(agent string) sessionParser {
	switch agent {
	case "hermes":
		return sessionParser{whole: parseHermes}
	case "alma":
		return sessionParser{whole: parseAlma}
	case "opencode", "zcode":
		return sessionParser{whole: parseOpenCode}
	case "dsh":
		return sessionParser{grow: parseDsh}
	case "cline":
		return sessionParser{grow: parseCline}
	case "grok":
		return sessionParser{grow: parseGrok}
	case "kimi":
		return sessionParser{grow: parseKimi}
	case "droid":
		return sessionParser{grow: parseDroid}
	case "cursor":
		return sessionParser{whole: parseCursor}
	case "codex":
		return sessionParser{line: codexBody}
	case "pi", "omp":
		return sessionParser{line: piParse}
	case "workbuddy", "codebuddy":
		return sessionParser{line: workbuddyLine}
	default:
		return sessionParser{line: claudeLine, claude: true}
	}
}

// parse reads a file on from where old left it, or from the start.
func parse(f file, old *state) *state { return parseWith(f, old, nil) }

// parseWith is parse with calls, when not nil, reading the file's calls on
// the same pass.
func parseWith(f file, old *state, calls *callsRide) *state {
	if calls != nil {
		defer calls.release()
	}
	// Summary caches contain aggregates only. After a restart, a changed
	// Codex file rebuilds its transient response index from the source.
	if f.agent == "codex" && old != nil && old.Codex == nil {
		old = nil
	}
	parser := parserFor(f.agent)
	if parser.whole != nil {
		return parser.whole(f)
	}
	if parser.grow != nil {
		return parser.grow(f, old)
	}
	// Every source using claudeLine must rebuild a missing transient window.
	if parser.claude && old != nil && old.Claude == nil {
		old = nil
	}

	headBytes := headOf(f.path)
	var s *state
	if old != nil && !packed(f.path) && f.size >= old.Size && old.Off <= f.size && sameHead(headBytes, old.Head, old.HeadSize) && old.ContentHash != "" && prefixHash(f.path, old.Size) == old.ContentHash {
		s = old.clone()
	} else {
		s = &state{}
	}
	s.Size, s.Mod = f.size, f.mod.UnixNano()
	s.Head, s.HeadSize = hashHead(headBytes), len(headBytes)
	s.ContentHash = prefixHash(f.path, f.size)
	var head func([]byte) bool
	if f.agent == "codex" {
		head = func(b []byte) bool { return codexHead(s, b, f.main) }
	}
	line := func(b []byte, _, _ int64) bool {
		parser.line(s, b, f.main)
		return true
	}
	if calls != nil {
		head, line = calls.ride(head, line)
	}
	off, err := scanAt(f.path, s.Off, head, line)
	if err == nil {
		s.Off = off
	}
	if calls != nil {
		calls.finish(off, err)
	}
	if isBuddy(f.agent) && s.Cwd == "" {
		workbuddyMeta(s, f.path)
	}
	return s
}

// maxLine is the longest line looked at; a longer one (an image pasted
// inline) is stepped over.
const maxLine = 32 << 20

// scan calls fn on each whole line of the file from off, and returns the
// offset after the last one. A line still being written is left for later.
func scan(path string, off int64, fn func([]byte)) (int64, error) {
	return scanHead(path, off, nil, fn)
}

// scanHead is scan with a look at each line's start first: head is given
// the line, or its first megabyte when it is longer, and a line it says no
// to is stepped over without being gathered (most of a Codex rollout's
// bytes are lines no one reads: compactions, tool output). A nil head
// wants every line.
// scanHook, when a test sets it, is told where each scan of a file starts.
var scanHook func(path string, off int64)

func scanHead(path string, off int64, head func([]byte) bool, fn func([]byte)) (int64, error) {
	return scanAt(path, off, head, func(b []byte, _, _ int64) bool { fn(b); return true })
}

// scanAt is scanHead telling fn where each line starts and ends in the file,
// and stopping when fn says so; what it returns is where the last line it
// handled ended.
// A compressed file's places are those of its lines decompressed; it is read
// from its start, what comes before off passed over.
func scanAt(path string, off int64, head func([]byte) bool, fn func(b []byte, start, end int64) bool) (int64, error) {
	if scanHook != nil {
		scanHook(path, off)
	}
	f, err := openLines(path)
	if err != nil {
		return off, err
	}
	defer f.Close()
	if s, ok := f.(io.Seeker); ok {
		if _, err := s.Seek(off, io.SeekStart); err != nil {
			return off, err
		}
	}
	r, release := lineReader(f, scanBuffer(f, off))
	defer release()
	if _, ok := f.(io.Seeker); !ok && off > 0 {
		// passed over through the reader the lines are read with, not a
		// copy's buffer of its own
		if _, err := r.Discard(int(off)); err != nil {
			if errors.Is(err, io.EOF) {
				return off, nil
			}
			return off, err
		}
	}
	pf, plain := f.(*os.File)
	var long []byte
	var kept *[]byte // long's buffer, from longLines
	defer func() { giveLong(kept, long) }()
	var parts blocks // a packed file's long line so far
	defer parts.release()
	var n int64 // bytes of the line so far
	skip, seen := false, false
	for {
		chunk, err := r.ReadSlice('\n')
		n += int64(len(chunk))
		if errors.Is(err, bufio.ErrBufferFull) {
			if !seen && head != nil {
				seen = true
				skip = skip || !head(chunk)
			}
			if plain && !skip && len(long) == 0 {
				// gathered in one buffer of the line's own size, not grown a
				// chunk at a time (~5 times the line allocated, #2515)
				if l := lineLen(pf, off, maxLine+r.Size()); l > maxLine+r.Size() {
					skip = true
				} else if l > cap(long) {
					kept, long = takeLong(l)
				}
			}
			switch {
			case skip || len(long)+parts.n+len(chunk) > maxLine:
				skip, long = true, long[:0]
				parts.release()
			case plain:
				long = append(long, chunk...)
			default:
				// a packed file's line has no length to read ahead: it is
				// kept in pooled blocks until its end, then gathered once
				// at its own size. Grown by append it allocated ~3.8 times
				// the line, most of what a cold read of packed rollouts
				// with pasted images allocated.
				parts.add(chunk)
			}
			continue
		}
		if err != nil {
			if errors.Is(err, io.EOF) {
				return off, nil
			}
			return off, err
		}
		if parts.n > 0 {
			if l := parts.n + len(chunk); l > cap(long) {
				kept, long = takeLong(l)
			}
			long = parts.appendTo(long[:0])
			parts.release()
		}
		b := chunk
		if len(long) > 0 {
			long = append(long, chunk...)
			b = long
		}
		if !skip {
			if b = bytes.TrimSpace(b); len(b) > 0 && (seen || head == nil || head(b)) {
				if !fn(b, off, off+n) {
					return off + n, nil
				}
			}
		}
		off += n
		n, skip, seen, long = 0, false, false, long[:0]
	}
}

// longLines keep the buffers scanAt gathers long lines in between scans:
// a line handed out from one is only good until the scan ends, as one
// from its read buffer is.
var longLines sync.Pool

// takeLong is an empty buffer of at least l bytes, from the pool when it
// has one that large (the one a scan had, too small, is let go).
func takeLong(l int) (*[]byte, []byte) {
	if p, ok := longShelf.take(func(p *[]byte) bool { return cap(*p) >= l }); ok {
		return p, (*p)[:0]
	}
	if p, _ := longLines.Get().(*[]byte); p != nil {
		if cap(*p) >= l {
			return p, (*p)[:0]
		}
		longLines.Put(p)
	}
	b := make([]byte, 0, l)
	return &b, b
}

// giveLong gives the buffer long was gathered in back to the pool.
func giveLong(p *[]byte, long []byte) {
	if p == nil {
		return
	}
	*p = long[:cap(long)]
	if poisonReleased {
		for i := range *p {
			(*p)[i] = 0xFF
		}
	}
	*p = (*p)[:0]
	if cap(*p) <= longShelved {
		// kept in place of a smaller one when the shelf is full
		out, kept, swapped := longShelf.give(p, func(h, p *[]byte) bool { return cap(*h) < cap(*p) })
		if !kept {
			longLines.Put(p)
		} else if swapped {
			longLines.Put(out)
		}
		return
	}
	longLines.Put(p)
}

// longShelf keeps long-line buffers up to longShelved bytes across garbage
// collections; a longer one, rare, is left to longLines.
var longShelf shelf[*[]byte]

const longShelved = 4 << 20

// blocks hold a long line read from a packed file until its end is found:
// copied into 1 MiB blocks kept between scans, so the line is allocated
// only once, at its own length.
type blocks struct {
	b []*[]byte
	n int // bytes held
}

var gatherBlocks = sync.Pool{New: func() any { b := make([]byte, 0, 1<<20); return &b }}

func (g *blocks) add(p []byte) {
	g.n += len(p)
	for len(p) > 0 {
		if len(g.b) == 0 || len(*g.b[len(g.b)-1]) == cap(*g.b[len(g.b)-1]) {
			g.b = append(g.b, gatherBlocks.Get().(*[]byte))
		}
		last := g.b[len(g.b)-1]
		k := min(len(p), cap(*last)-len(*last))
		*last = append(*last, p[:k]...)
		p = p[k:]
	}
}

func (g *blocks) appendTo(dst []byte) []byte {
	for _, b := range g.b {
		dst = append(dst, *b...)
	}
	return dst
}

func (g *blocks) release() {
	for _, b := range g.b {
		*b = (*b)[:0]
		gatherBlocks.Put(b)
	}
	g.b, g.n = g.b[:0], 0
}

// lineLen is the length, its newline included, of the line at off in f,
// read ahead without moving f: 0 when it has no end yet, and past most
// (but no further) for a longer one.
func lineLen(f *os.File, off int64, most int) int {
	buf := make([]byte, 64<<10)
	n := 0
	for n <= most {
		k, err := f.ReadAt(buf, off+int64(n))
		if i := bytes.IndexByte(buf[:k], '\n'); i >= 0 {
			return n + i + 1
		}
		n += k
		if err != nil {
			return 0
		}
	}
	return n
}

// scanBuffer is the read buffer a reader of f needs for what is left of it
// from off: 1 MiB at most, and no more than that rest of a plain file.
// Every line reader of a session file sizes its buffer by it. Every scan
// took 1 MiB, and a start's read of ~4,600 mostly small session files
// (each read again from where it was left) allocated ~9 GB of them, which
// the GC's heap target followed up (#2776). A line longer than the buffer
// (a file grown since) is still read whole, through scanAt's long line.
func scanBuffer(f io.Reader, off int64) int {
	const most, least = 1 << 20, 16 << 10
	if pf, ok := f.(*os.File); ok {
		if info, err := pf.Stat(); err == nil {
			return int(min(most, max(least, info.Size()-off+1)))
		}
	}
	return most
}

// lineReaders keep scanAt's read buffers between scans, one pool per
// power-of-two size from scanBuffer's 16 KiB to its 1 MiB. Every scan
// allocated its own: a live magpie's start read ~3,200 Claude Code subagent
// files for their first lines (ccTask), most over 1 MiB, and with the
// session and call reads scanAt allocated 6.3 GB in 48 minutes, 3.8 GB of
// it these buffers, while ~157 MB was live. A pool keeps about one buffer
// per reader running at once, and lets idle ones go at the next GCs.
var lineReaders [7]sync.Pool

// lineShelves keep a few of each first, across garbage collections.
var lineShelves [7]shelf[*bufio.Reader]

// poisonReleased, set by the tests, fills a buffer given back with 0xFF, so
// a reader that kept a line past its scan reads garbage and fails.
var poisonReleased = false

// lineReader is a reader of f with a buffer of at least size bytes (at
// most 1 MiB); release gives the buffer back. A line handed out from it is
// only good until release: one kept longer is copied.
func lineReader(f io.Reader, size int) (*bufio.Reader, func()) {
	class := 0
	for class < len(lineReaders)-1 && 16<<10<<class < size {
		class++
	}
	r, _ := lineShelves[class].take(nil)
	if r == nil {
		r, _ = lineReaders[class].Get().(*bufio.Reader)
	}
	if r == nil {
		r = bufio.NewReaderSize(nil, 16<<10<<class)
	}
	r.Reset(f)
	return r, func() {
		if poisonReleased {
			r.Reset(poison{})
			r.Peek(r.Size())
		}
		r.Reset(nil)
		if _, kept, _ := lineShelves[class].give(r, nil); !kept {
			lineReaders[class].Put(r)
		}
	}
}

type poison struct{}

func (poison) Read(b []byte) (int, error) {
	for i := range b {
		b[i] = 0xFF
	}
	return len(b), nil
}

// packed says whether a session file is compressed: a Codex rollout the
// Codex app has packed (.zst), a dsh session (.zstd). A packed rollout is
// written whole, so it is read whole again whenever it changes.
func packed(path string) bool {
	return strings.HasSuffix(path, zstSuffix) || strings.HasSuffix(path, ".zstd")
}

// openLines opens a session file to read its lines, a packed one decompressed.
func openLines(path string) (io.ReadCloser, error) {
	f, err := os.Open(path)
	if err != nil || !packed(path) {
		return f, err
	}
	d, ok := streamShelf.take(nil)
	if !ok {
		d = streams.Get().(*zstd.Decoder)
	}
	if err := d.Reset(f); err != nil {
		f.Close()
		return nil, err
	}
	return &zstdFile{d: d, f: f}, nil
}

// streams holds decoders between packed reads. A decoder made for each
// read allocated its window (a packed rollout's is 2 MiB, held in 3 MiB at
// low memory) and its block buffers every time: 75 GB of a live magpie's
// first read of 11,591 rollouts. One kept here keeps them, and is let go
// with the pool's other idle ones by the garbage collector. streamShelf
// keeps a few first, across collections.
var streamShelf shelf[*zstd.Decoder]

var streams = sync.Pool{New: func() any {
	d, err := zstd.NewReader(nil, zstd.WithDecoderConcurrency(1), zstd.WithDecoderLowmem(true))
	if err != nil {
		panic(err) // only for options it doesn't know
	}
	return d
}}

// zstdFile is a packed file's lines, closed with the file. Its decoder goes
// back to the pool at Close, which a read after can't reach.
type zstdFile struct {
	d *zstd.Decoder
	f *os.File
}

func (z *zstdFile) Read(p []byte) (int, error) {
	if z.d == nil {
		return 0, os.ErrClosed
	}
	return z.d.Read(p)
}

func (z *zstdFile) Close() error {
	if z.d == nil {
		return os.ErrClosed
	}
	z.d.Reset(nil)
	if _, kept, _ := streamShelf.give(z.d, nil); !kept {
		streams.Put(z.d)
	}
	z.d = nil
	return z.f.Close()
}

// tsAt reads the time of a line from its first (or last) "timestamp" key,
// without decoding the rest of it.
func tsAt(b []byte, last bool) time.Time {
	key := []byte(`"timestamp":"`)
	i := bytes.Index(b, key)
	if last {
		i = bytes.LastIndex(b, key)
	}
	if i < 0 {
		return time.Time{}
	}
	rest := b[i+len(key):]
	j := bytes.IndexByte(rest, '"')
	if j < 0 || j > 40 {
		return time.Time{}
	}
	t, _ := time.Parse(time.RFC3339Nano, string(rest[:j]))
	return t
}

var tag = regexp.MustCompile(`</?[A-Za-z][\w-]*[^<>]*>`)

// untagged is a message the agent or a harness wrapped in tags, as words.
func untagged(s string) string { return title(tag.ReplaceAllString(s, " ")) }

// title is a prompt made one short line: each run of ASCII white space (as
// regexp's \s) one space, trimmed, and cut at 160 characters. It reads only
// as far as it keeps, and one past: a pasted file as a prompt went through
// a regexp and into runes whole, 0.45 GB of a cold start's first 2 minutes.
func title(s string) string {
	const most = 160
	s = strings.TrimLeftFunc(s, unicode.IsSpace)
	b := make([]byte, 0, min(len(s), 4*most))
	n, in, i := 0, false, 0 // runes kept; in a run of \s
	for ; i < len(s) && n < most; i++ {
		switch c := s[i]; c {
		case '\t', '\n', '\f', '\r', ' ':
			if !in {
				b, n, in = append(b, ' '), n+1, true
			}
		default:
			_, w := utf8.DecodeRuneInString(s[i:])
			b, n, in = append(b, s[i:i+w]...), n+1, false
			i += w - 1
		}
	}
	if strings.IndexFunc(s[i:], func(r rune) bool { return !unicode.IsSpace(r) }) < 0 {
		return strings.TrimRightFunc(string(b), unicode.IsSpace)
	}
	// as []rune made it: each byte not UTF-8 one U+FFFD
	return strings.TrimSpace(string([]rune(string(b)))) + "…"
}

// ---- pricing ----------------------------------------------------------------

var dated = regexp.MustCompile(`-\d{8}$`)

// priceOf prices a model as a session names it: one through magpie as
// "<provider>/<model>" at the price the gateway counts it at — what the user
// set for that provider and model, else that provider's own list price, else
// its maker's on models.dev — and a bare id only ever at its maker's, which is
// a different question from what one provider charges.
//
// The settings are the ones passed in rather than read here: a listing reads
// them once and prices every model of it against that same copy, so a
// listing is one snapshot of the prices and reads the file once, not once
// per model.
func priceOf(s settings.Settings, model string) (catalog.Price, bool) {
	m := strings.TrimSpace(model)
	if m == "" {
		return catalog.Price{}, false
	}
	if pid, rest, ok := strings.Cut(m, "/"); ok {
		if pr, ok := provider.EffectivePriceIn(s, pid, rest); ok {
			return pr, true
		}
	}
	bare := strings.ToLower(m[strings.LastIndexByte(m, '/')+1:])
	// what the user said the model costs from any provider (*/model): the
	// only price a bare id, or one models.dev has stopped listing, can take
	// from them
	if p, ok := s.ModelPrices[provider.AnyPriceKey(bare)]; ok {
		if pr, bad := p.Price(); bad == "" {
			catalog.OneHourFor(bare, &pr)
			return pr, true
		}
	}
	// a Grok id at an effort (grok-4.7-high) at its model's price, and a
	// version spelled with a dot (Copilot's claude-opus-4.6) at its maker's
	// claude-opus-4-6
	for _, id := range []string{bare, dated.ReplaceAllString(bare, ""), provider.PricedName(bare)} {
		if pr, ok := catalog.PricedBy(makers(id), id); ok {
			return pr, true
		}
	}
	// Grok Build's fast variant (grok-4.7-build-fast), which only the Grok
	// CLI names, at twice its model's price, as a Grok account's call on it
	// is (TradingLife on Discord)
	if base, ok := provider.GrokBuildFast(bare); ok {
		if pr, ok := catalog.PricedBy(makers(base), base); ok {
			return pr.Times(provider.GrokFastTimes), true
		}
	}
	return catalog.Price{}, false
}

// makers are the models.dev providers that make a model of this id.
func makers(id string) []string {
	for _, m := range []struct{ prefix, provider string }{
		{"claude", "anthropic"}, {"gpt", "openai"}, {"o1", "openai"}, {"o3", "openai"}, {"o4", "openai"},
		{"codex", "openai"}, {"gemini", "google"}, {"deepseek", "deepseek"}, {"grok", "xai"},
		{"glm", "zai"}, {"kimi", "moonshotai"}, {"qwen", "alibaba"}, {"mistral", "mistral"},
		{"devstral", "mistral"}, {"minimax", "minimax"},
	} {
		if strings.HasPrefix(id, m.prefix) {
			return []string{m.provider}
		}
	}
	return nil
}

// ---- resuming ---------------------------------------------------------------

var safeID = regexp.MustCompile(`^[0-9A-Za-z_-]+$`)
var safeOmpProfile = regexp.MustCompile(`^[a-z0-9][a-z0-9._-]{0,63}$`)

// ResumeCommand is the shell line that picks a session up again in its
// folder, or "" for an id that isn't plain.
func ResumeCommand(agent, id, cwd string) string { return resumeCommand("", agent, id, cwd, "") }

// resumeCommand is ResumeCommand for a session in a WSL distro too, when
// distro is set: see wslResume.
func resumeCommand(distro, agent, id, cwd, path string) string {
	if !safeID.MatchString(id) {
		return ""
	}
	var run string
	switch agent {
	case "claude":
		run = "claude --resume " + id
	case "codex":
		run = "codex resume " + id
	case "opencode":
		run = "opencode --session " + id
	case "pi":
		run = "pi --session " + id
	case "cline":
		run = "cline --id " + id
	case "qoder":
		run = "qodercli --resume " + id
	case "qoder-cn":
		run = "qoderclicn --resume " + id
	case "grok":
		run = "grok --resume " + id
	case "codebuddy":
		// its CLI's -r, --resume [sessionId], one bin of the package's
		// three (codebuddy, cbc, codebuddy-code)
		run = "codebuddy --resume " + id
	case "omp":
		run = "omp --resume " + id
		if profile := ompProfile(path); profile != "" {
			if !safeOmpProfile.MatchString(profile) {
				return ""
			}
			run = "omp --profile " + profile + " --resume " + id
		}
	case "droid":
		run = "droid --resume " + id
	case "kimi":
		run = "kimi --session " + id
	case "cursor":
		// its chats are looked up by the folder it runs in
		if cwd == "" {
			return ""
		}
		run = "cursor-agent --resume " + id
	default:
		return ""
	}
	if distro != "" {
		return wslResume(distro, cwd, run)
	}
	if cwd == "" {
		return run
	}
	if runtime.GOOS == "windows" {
		return "Set-Location -LiteralPath '" + strings.ReplaceAll(cwd, "'", "''") + "'; " + run
	}
	return "cd " + shellQuote(cwd) + " && " + run
}

func shellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

// Find is a session by agent and id: a listed one, else one Get finds
// under agent:id. One nothing was said in is found too, as ListAgent lists
// it: the Sessions page shows it, and its conversation, Markdown and
// Terminal open from it as from any other (#2273).
func Find(agent, id string) (Session, bool) {
	for _, s := range List(0) {
		if s.Agent == agent && s.ID == id {
			return s, true
		}
	}
	if s, ok, found := get(agent + ":" + id); found && s.ID == id {
		if !ok {
			listed(&s)
		}
		return s, true
	}
	return Session{}, false
}

// Titles is the title of each session named by key (agent:id, as Get takes
// it) that has its files on this computer: the name it was given, else the
// agent's own title, else its first prompt. A key with no files, or a
// session with no title, is left out. Desktop's local metadata can name a
// session without its CLI transcript. The Routing page uses this too (#1293).
func Titles(keys []string) map[string]string {
	out := map[string]string{}
	if len(keys) == 0 {
		return out
	}
	want := make(map[string]bool, len(keys))
	var desktopIDs, piDesktopIDs []string
	for _, k := range keys {
		want[k] = true
		if id, ok := strings.CutPrefix(k, "claude-desktop:"); ok {
			desktopIDs = append(desktopIDs, id)
			continue
		}
		// PI-Desktop names its chats in pi.sqlite, which none of their files
		// holds (#1967); a request it sent without a User-Agent of its own
		// was counted as Pi's, so both of its names are looked up
		if agent, id, ok := strings.Cut(k, ":"); ok && isPIDesktop(agent) {
			piDesktopIDs = append(piDesktopIDs, id)
		}
	}
	for id, name := range DesktopTitles(desktopIDs) {
		out["claude-desktop:"+id] = name
	}
	if len(piDesktopIDs) > 0 {
		names := PIDesktopTitles(piDesktopIDs)
		for _, k := range keys {
			if agent, id, ok := strings.Cut(k, ":"); ok && isPIDesktop(agent) {
				if name := names[id]; name != "" {
					out[k] = name
				}
			}
		}
	}
	mu.Lock()
	defer mu.Unlock()
	loadCache()
	defer useDBs()()
	files := allFiles()
	groups := map[string][]file{}
	var fs []file
	for _, f := range files {
		if want[f.key] {
			groups[f.key] = append(groups[f.key], f)
			fs = append(fs, f)
		}
	}
	if len(fs) == 0 {
		return out
	}
	refresh(fs, files)
	parents := codexTaskParentFiles(fs, files)
	if len(parents) > 0 {
		refresh(parents, files)
	}
	titleFiles := append(append([]file(nil), fs...), parents...)
	taskTitles := codexSubagentTaskTitles(titleFiles)
	identities := codexIdentitySnapshot(fs)
	price := pricer()
	for k, g := range groups {
		if s, ok := assembleListed(g, price); ok {
			setSessionCodexIdentity(&s, identities)
			setSessionCodexTaskTitle(&s, taskTitles)
			if s.Title == "" {
				continue
			}
			out[k] = s.Title
		}
	}
	return out
}

// Get is the session whose files are grouped under key (a Summary's Key),
// however long ago it was at work.
func Get(key string) (Session, bool) {
	s, ok, _ := get(key)
	return s, ok
}

// get is Get, and whether key has files at all: ok is false for a session
// nothing was said in, which found still says is there.
func get(key string) (s Session, ok, found bool) {
	mu.Lock()
	defer mu.Unlock()
	loadCache()
	defer useDBs()()
	files := allFiles()
	var fs []file
	for _, f := range files {
		if f.key == key {
			fs = append(fs, f)
		}
	}
	if len(fs) == 0 {
		return Session{}, false, false
	}
	var identities map[string]CodexIdentity
	var taskTitles map[codexTaskTitle]string
	if strings.HasPrefix(key, "codex:") {
		var codexFiles []file
		for _, f := range files {
			if f.agent == "codex" {
				codexFiles = append(codexFiles, f)
			}
		}
		refresh(codexFiles, files)
		identities = codexIdentitySnapshot(codexFiles)
		taskTitles = codexSubagentTaskTitles(codexFiles)
	} else {
		refresh(fs, files)
	}
	s, ok = assemble(fs, pricer())
	setSessionCodexIdentity(&s, identities)
	setSessionCodexTaskTitle(&s, taskTitles)
	return s, ok, true
}
