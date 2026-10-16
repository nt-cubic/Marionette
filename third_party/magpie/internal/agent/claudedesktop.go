package agent

// Claude Desktop runs on a third-party gateway in its "3p" deployment mode:
// no Anthropic sign-in, and its Code and Cowork tabs work on any plan (a
// Free account signed in sees only "Get Claude Code" there). The mode is
// claude_desktop_config.json's deploymentMode, in its own folder and in
// Claude-3p beside it; the gateway is a profile in Claude-3p/configLibrary,
// the one _meta.json applies:
//
//	Claude/claude_desktop_config.json      {"deploymentMode":"3p",…}
//	Claude-3p/claude_desktop_config.json   {"deploymentMode":"3p",…}
//	Claude-3p/configLibrary/<id>.json      {"inferenceProvider":"gateway",
//	  "inferenceGatewayBaseUrl":"http://127.0.0.1:3425",
//	  "inferenceGatewayApiKey":"magpie-claude-desktop",
//	  "inferenceGatewayAuthScheme":"bearer",
//	  "disableDeploymentModeChooser":true,"coworkEgressAllowedHosts":["*"]}
//	Claude-3p/configLibrary/_meta.json     {"appliedId":"<id>",
//	  "entries":[…,{"id":"<id>","name":"magpie"}]}
//
// as CC Switch writes them (claude_desktop_config.rs). The folders are in
// ~/Library/Application Support on macOS, %LOCALAPPDATA% on Windows (the
// MSIX package's LocalCache\Local for a packaged Desktop) and
// $XDG_CONFIG_HOME on Linux, as internal/desktopdir finds them. A
// packaged Desktop gets all of it twice, in the package's folders and the
// real ones: up to 2.x it reads the package's copy, from 2.31226 the real
// %LOCALAPPDATA%\Claude-3p (desktopdir.Sets). With no inferenceModels Desktop lists the
// gateway's /v1/models, which the gateway gives it by ids it keeps (see
// gateway/desktop.go). Desktop reads all this at start-up only.

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/desktopdir"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
)

// desktopProfileID is magpie's profile in Desktop's configLibrary: a
// UUID like the ones Desktop makes, its last part "magpie" in hex.
const desktopProfileID = "00000000-0000-4000-8000-6d6167706965"

// desktopPaths are the files and folders magpie writes for Claude Desktop.
type desktopPaths struct {
	key                 string // the stash's keys end with it: "" or ".package"
	dir, dir3p          string // Claude, Claude-3p
	config, config3p    string // their claude_desktop_config.json
	library, meta, prof string // configLibrary, its _meta.json, magpie's profile
}

func desktopPathsOf(dir, dir3p string) desktopPaths {
	lib := filepath.Join(dir3p, "configLibrary")
	return desktopPaths{
		dir: dir, dir3p: dir3p,
		config:   filepath.Join(dir, "claude_desktop_config.json"),
		config3p: filepath.Join(dir3p, "claude_desktop_config.json"),
		library:  lib, meta: filepath.Join(lib, "_meta.json"), prof: filepath.Join(lib, desktopProfileID+".json"),
	}
}

// desktopDirs are Desktop's Claude and Claude-3p folders on goos: the ones
// magpie writes deploymentMode and its profile into (desktopdir.Find).
func desktopDirs(goos, home string, getenv func(string) string) (string, string) {
	d := desktopdir.Find(goos, home, getenv)
	return d.Mode, d.ThreeP
}

// desktopSets are those of every set of Desktop's folders, Find's first:
// for a packaged Desktop the package's and the real ones, each with its
// own stash.
func desktopSets(goos, home string, getenv func(string) string) []desktopPaths {
	var out []desktopPaths
	for _, d := range desktopdir.Sets(goos, home, getenv) {
		p := desktopPathsOf(d.Mode, d.ThreeP)
		if d.Package != "" {
			p.key = ".package"
		}
		out = append(out, p)
	}
	return out
}

// DesktopConfigs3p are the claude_desktop_config.json files Claude Desktop
// reads in its 3p mode: there its whole userData is Claude-3p, its MCP
// servers too (%LOCALAPPDATA%\Claude-3p on Windows, and the MSIX package's;
// Claude-3p beside Claude elsewhere).
func DesktopConfigs3p(home string) []string {
	var out []string
	for _, p := range desktopSets(desktopdir.OS, home, os.Getenv) {
		out = append(out, p.config3p)
	}
	return out
}

func claudeDesktop(home string) *Agent {
	sets := desktopdir.Sets(desktopdir.OS, home, os.Getenv)
	ps := desktopSets(desktopdir.OS, home, os.Getenv)
	p := ps[0]
	return &Agent{
		ID: "claude-desktop", Name: "Claude Desktop", Icon: "claude-color", Aliases: []string{"claude-app"},
		Dir: p.dir, Path: p.config,
		detect: func() bool {
			// its own data (%APPDATA%\Claude on Windows, or the MSIX
			// package's), the folder magpie writes or Claude-3p
			for _, s := range sets {
				for _, d := range s.All() {
					if isDir(d) {
						return true
					}
				}
			}
			// installed on Windows and not yet opened: none of them yet
			return desktopdir.Installed(desktopdir.OS, home, os.Getenv)
		},
		Notice: func() string {
			if desktopWiredAny(ps) {
				return noticeClaudedesktop1.String()
			}
			return noticeClaudedesktop2.String()
		},
		Check: func() string {
			if !desktopWiredAny(ps) {
				return ""
			}
			for _, p := range desktopRead(ps) {
				if msg := desktopCheck(p); msg != "" {
					if len(ps) > 1 {
						msg += " (" + p.dir3p + ")"
					}
					return msg
				}
			}
			return ""
		},
		Fields: append([]Field{{
			Key: "provider", Label: "provider",
			Get: func() string {
				if desktopWiredAny(ps) {
					return magpieID
				}
				return ""
			},
			Set: func(v string) error {
				on := desktopOn
				if v == "" {
					on = desktopOff
				}
				desktopAdoptStash(ps)
				for _, p := range ps {
					if err := on(p); err != nil {
						return err
					}
				}
				// its Code tab is Claude Code, told what Desktop's ids for
				// magpie's models can do while that one runs on magpie
				_ = claude(home).Sync()
				return nil
			},
			Options: func(map[string]string) []Option {
				return []Option{{Value: magpieID, Label: "magpie", Icon: "magpie",
					Note: "Desktop's third-party gateway: Code and Cowork on magpie's models, no Anthropic sign-in (restart Desktop)"}}
			},
		}}, append(desktopTierFields(ps), desktopSubagentField(ps), desktopSubagentEffortField(ps))...),
	}
}

// desktopRead are the sets of Desktop's folders whose wiring counts: the
// one whose Claude-3p Desktop ran in last, which is where it reads
// deploymentMode and the gateway from, or every set when it has run in
// none. Magpie writes both of a packaged Desktop's sets, not knowing the
// build: 2.31226's MSIX keeps Claude-3p out of the package
// (ExcludedDirectory), so the package's copy is one Desktop no longer
// reads, and one gone from there (emptied by an update) isn't Desktop off
// magpie (dumplings on Discord: "配置已变更" while Desktop ran on magpie).
func desktopRead(ps []desktopPaths) []desktopPaths {
	var newest time.Time
	var at []desktopPaths
	for _, p := range ps {
		if t := desktopdir.LastRun(p.dir3p); t.After(newest) {
			newest, at = t, []desktopPaths{p}
		}
	}
	if at == nil {
		return ps
	}
	return at
}

// desktopCheck is what keeps one set of Desktop's folders off magpie's
// gateway, "" for nothing.
func desktopCheck(p desktopPaths) string {
	m, _ := edit.GetJSON(p.config, "deploymentMode")
	m3, _ := edit.GetJSON(p.config3p, "deploymentMode")
	if m != "3p" || m3 != "3p" {
		return "Claude Desktop's deploymentMode (claude_desktop_config.json) is no longer 3p, so it signs in with Anthropic rather than using magpie"
	}
	if id, _ := edit.GetJSON(p.meta, "appliedId"); id != desktopProfileID {
		return "Claude Desktop uses another gateway configuration now (appliedId in configLibrary/_meta.json), not magpie's"
	}
	return wiringOff("Claude Desktop", p.prof, func(k string) (string, bool) { return edit.GetJSON(p.prof, k) },
		"inferenceGatewayBaseUrl", gateway.URL(), "inferenceGatewayApiKey", agentKey("claude-desktop"))
}

// desktopTierFields pick the model each of Claude Code's tiers runs on in
// Desktop's Code tab, its subagents' sonnet, haiku or opus among them
// (gateway.DesktopTiers). Unset, a Claude model magpie serves stands in for
// its own tier, else the chat's model does. Desktop reads them from the
// gateway's /v1/models when it starts.
func desktopTierFields(ps []desktopPaths) []Field {
	var fields []Field
	for _, tier := range gateway.DesktopTierNames {
		fields = append(fields, Field{
			Key: tier, Label: tier, Quiet: true,
			Get: func() string { return gateway.DesktopTiers()[tier] },
			Set: func(v string) error {
				v = gateway.DesktopCatalogID(v)
				if v != "" && !isMagpie(v) {
					return fmt.Errorf("%s: %q is not a model magpie serves", tier, v)
				}
				return gateway.SetDesktopTier(tier, v)
			},
			Options: func(map[string]string) []Option {
				if !desktopWiredAny(ps) {
					return nil
				}
				return viaMagpie("claude-desktop", "")
			},
		})
	}
	return fields
}

// desktopSubagentField picks the model every subagent turn in Desktop's
// Code tab and Cowork runs on, whatever its lead or tier asked for. It
// writes nothing to Desktop's config, which has no key for it, and the
// Claude Code it runs ignores CLAUDE_CODE_SUBAGENT_MODEL in ~/.claude
// while Desktop manages its provider: it is magpie's setting
// (settings.DesktopSubagentModel), which the gateway reads on each
// request (gateway desktopSubagentPick), so it is Live and a change needs
// no restart of Desktop. Labelled as Codex's "subagent model", it gets the
// same square on the Agents page. A Desktop alias picked is kept by its
// catalog id.
func desktopSubagentField(ps []desktopPaths) Field {
	return Field{
		Key: "subagent_model", Label: "subagent model", Quiet: true, Live: true,
		Get: provider.DesktopSubagentModel,
		Set: func(v string) error {
			return provider.SetDesktopSubagentModel(gateway.DesktopCatalogID(v))
		},
		Options: func(map[string]string) []Option {
			if !desktopWiredAny(ps) {
				return nil
			}
			return viaMagpie("claude-desktop", "")
		},
	}
}

// Desktop's subagent effort is a live gateway setting, like its subagent
// model. With no model picked, a subagent may use any of Desktop's models,
// so all levels are offered and the gateway fits each to the actual model.
func desktopSubagentEffortField(ps []desktopPaths) Field {
	return Field{
		Key: "subagent_effort", Label: "subagent effort", Quiet: true, Live: true,
		Get: provider.DesktopSubagentEffort,
		Set: provider.SetDesktopSubagentEffort,
		Options: func(map[string]string) []Option {
			if !desktopWiredAny(ps) {
				return nil
			}
			target := provider.DesktopSubagentModel()
			if target == "" {
				return static(provider.MemberEfforts...)
			}
			for _, e := range provider.Served() {
				if e.ID != target {
					continue
				}
				if provider.Reasonless(e.Provider.ID, e.Model) {
					return nil
				}
				var levels []string
				for _, level := range e.Efforts {
					if slices.Contains(provider.MemberEfforts, level) {
						levels = append(levels, level)
					}
				}
				if len(levels) > 0 {
					return static(levels...)
				}
			}
			return static(provider.MemberEfforts...)
		},
	}
}

// desktopWired: _meta.json lists magpie's profile.
func desktopWired(p desktopPaths) bool {
	entries, _, err := desktopEntries(p.meta)
	return err == nil && slices.ContainsFunc(entries, desktopOurs)
}

// desktopWiredAny: any set of Desktop's folders is on magpie.
func desktopWiredAny(ps []desktopPaths) bool { return slices.ContainsFunc(ps, desktopWired) }

// desktopEntries reads _meta.json's entries, and whether the file is there.
func desktopEntries(meta string) ([]json.RawMessage, bool, error) {
	b, err := edit.Read(meta)
	if err != nil || b == nil {
		return nil, false, err
	}
	if len(bytes.TrimSpace(b)) == 0 {
		return nil, true, nil
	}
	var doc map[string]json.RawMessage
	if err := json.Unmarshal(b, &doc); err != nil {
		return nil, true, errors.New(meta + ": not a JSON object")
	}
	var entries []json.RawMessage
	if raw, ok := doc["entries"]; ok {
		if err := json.Unmarshal(raw, &entries); err != nil {
			return nil, true, errors.New(meta + ": entries is not a list")
		}
	}
	return entries, true, nil
}

func desktopEntryID(raw json.RawMessage) string {
	var e struct {
		ID string `json:"id"`
	}
	json.Unmarshal(raw, &e)
	return e.ID
}

func desktopOurs(raw json.RawMessage) bool { return desktopEntryID(raw) == desktopProfileID }

// desktopObject: the file is missing, empty or a JSON object — one magpie
// can edit without losing anything.
func desktopObject(path string) error {
	b, err := edit.Read(path)
	if err != nil || len(bytes.TrimSpace(b)) == 0 {
		return err
	}
	var m map[string]json.RawMessage
	if json.Unmarshal(b, &m) != nil {
		return errors.New(path + ": not a JSON object")
	}
	return nil
}

// The stash keeps what Desktop had before magpie: each file's
// deploymentMode ("" when it had none), the profile applied, and the
// files and folders magpie made, so switching off puts all of it back.
// It also keeps magpie's profile as it was when switched off, the
// settings the user made in it from Desktop's window among them, so
// switching on again brings them back (#2035).
// A package's folders keep theirs under the same keys ending in ".package".
const (
	desktopStashed = "claude-desktop.on"
	desktopMode    = "claude-desktop.mode"
	desktopMode3p  = "claude-desktop.mode3p"
	desktopApplied = "claude-desktop.applied"
	desktopMade    = "claude-desktop.made"
	desktopProfile = "claude-desktop.profile"
)

var desktopStashKeys = []string{desktopStashed, desktopMode, desktopMode3p, desktopApplied, desktopMade, desktopProfile}

// desktopAdoptStash moves the stash a packaged Desktop's folders got
// before they had keys of their own (when magpie wrote only one set) onto
// the package's keys: the stash is theirs when the package's folders are
// on magpie and the real ones aren't.
func desktopAdoptStash(ps []desktopPaths) {
	var pkg, real *desktopPaths
	for i := range ps {
		if ps[i].key == ".package" {
			pkg = &ps[i]
		} else {
			real = &ps[i]
		}
	}
	if pkg == nil || real == nil || !desktopWired(*pkg) || desktopWired(*real) {
		return
	}
	m := stashLoad()
	if m[desktopStashed] == "" || m[desktopStashed+pkg.key] != "" {
		return
	}
	kv := map[string]string{}
	for _, k := range desktopStashKeys {
		kv[k+pkg.key], kv[k] = m[k], ""
	}
	stash(kv)
}

// desktopOn points Claude Desktop at the gateway: deploymentMode 3p in both
// config files, magpie's profile written and applied. Every other key, and
// every other profile, stays as it is.
func desktopOn(p desktopPaths) error {
	// nothing is written unless every file can be
	for _, f := range []string{p.config, p.config3p, p.prof} {
		if err := desktopObject(f); err != nil {
			return err
		}
	}
	entries, _, err := desktopEntries(p.meta)
	if err != nil {
		return err
	}
	wired := slices.ContainsFunc(entries, desktopOurs)
	if !wired {
		var made []string
		for _, f := range []string{p.dir, p.config, p.dir3p, p.config3p, p.library, p.meta} {
			if _, err := os.Stat(f); err != nil {
				made = append(made, f)
			}
		}
		mode, _ := edit.GetJSON(p.config, "deploymentMode")
		mode3p, _ := edit.GetJSON(p.config3p, "deploymentMode")
		applied, _ := edit.GetJSON(p.meta, "appliedId")
		k := p.key
		forget(desktopMode+k, desktopMode3p+k, desktopApplied+k, desktopMade+k)
		stash(map[string]string{desktopStashed + k: "1", desktopMode + k: mode, desktopMode3p + k: mode3p,
			desktopApplied + k: applied, desktopMade + k: strings.Join(made, "\n")})
	}

	// the profile: the gateway's address and key set, Desktop's own
	// settings in it (the user's, from its window) kept, and those it had
	// when switched off put back (#2035)
	if b, _ := edit.Read(p.prof); len(bytes.TrimSpace(b)) == 0 {
		if err := os.MkdirAll(p.library, 0o755); err != nil {
			return err
		}
		was := []byte("{}\n")
		if kept := unstash(desktopProfile + p.key); kept != "" && desktopObjectBytes([]byte(kept)) {
			was = []byte(kept)
		} else if own := desktopOwnGatewayProfile(p); own != nil {
			was = own
		}
		if err := os.WriteFile(p.prof, was, 0o600); err != nil {
			return err
		}
	}
	kvs := []edit.KV{
		{Path: "inferenceProvider", Value: "gateway"},
		{Path: "inferenceGatewayBaseUrl", Value: gateway.URL()},
		{Path: "inferenceGatewayApiKey", Value: agentKey("claude-desktop")},
		{Path: "inferenceGatewayAuthScheme", Value: "bearer"},
	}
	// policies seeded once: the user may tighten them in Desktop
	if _, ok := edit.GetJSON(p.prof, "disableDeploymentModeChooser"); !ok {
		kvs = append(kvs, edit.KV{Path: "disableDeploymentModeChooser", Value: true})
	}
	if _, ok := edit.GetJSON(p.prof, "coworkEgressAllowedHosts"); !ok {
		kvs = append(kvs, edit.KV{Path: "coworkEgressAllowedHosts", Value: []string{"*"}})
	}
	if err := edit.SetJSON(p.prof, kvs...); err != nil {
		return err
	}

	// _meta.json: magpie's entry (where it was, if it was), applied
	if !wired {
		entry, _ := json.Marshal(map[string]string{"id": desktopProfileID, "name": "magpie"})
		entries = append(entries, entry)
		if err := edit.SetJSON(p.meta, edit.KV{Path: "entries", Value: entries}); err != nil {
			return err
		}
	}
	if err := edit.SetJSON(p.meta, edit.KV{Path: "appliedId", Value: desktopProfileID}); err != nil {
		return err
	}

	// the mode last: until here Desktop still starts as it did
	for _, f := range []string{p.config3p, p.config} {
		if err := edit.SetJSON(f, edit.KV{Path: "deploymentMode", Value: "3p"}); err != nil {
			return err
		}
	}
	return nil
}

// desktopOff puts Claude Desktop back as it was before magpie: each
// deploymentMode as it was (gone where there was none), the profile it had
// applied, magpie's profile and entry removed, and the files and folders
// magpie made gone again if nothing else went into them.
func desktopOff(p desktopPaths) error {
	entries, _, err := desktopEntries(p.meta)
	if err != nil {
		return err
	}
	k := p.key
	stashed := unstash(desktopStashed+k) != ""
	mode, mode3p, applied := unstash(desktopMode+k), unstash(desktopMode3p+k), unstash(desktopApplied+k)
	var made []string
	if m := unstash(desktopMade + k); m != "" {
		made = strings.Split(m, "\n")
	}
	if !stashed && !slices.ContainsFunc(entries, desktopOurs) {
		return nil
	}

	for f, was := range map[string]string{p.config: mode, p.config3p: mode3p} {
		if cur, _ := edit.GetJSON(f, "deploymentMode"); cur != "3p" {
			continue // changed since, by Desktop or the user: theirs
		}
		if was != "" {
			err = edit.SetJSON(f, edit.KV{Path: "deploymentMode", Value: was})
		} else {
			err = edit.DelJSON(f, "deploymentMode")
		}
		if err != nil {
			return err
		}
	}

	if slices.ContainsFunc(entries, desktopOurs) {
		entries = slices.DeleteFunc(entries, desktopOurs)
		if len(entries) == 0 && slices.Contains(made, p.meta) {
			err = edit.DelJSON(p.meta, "entries")
		} else {
			if entries == nil {
				entries = []json.RawMessage{}
			}
			err = edit.SetJSON(p.meta, edit.KV{Path: "entries", Value: entries})
		}
		if err != nil {
			return err
		}
	}
	if id, _ := edit.GetJSON(p.meta, "appliedId"); id == desktopProfileID {
		// the profile applied before, if it is still there, else the first
		// one left, else none
		next := ""
		if applied != "" && applied != desktopProfileID && slices.ContainsFunc(entries, func(e json.RawMessage) bool { return desktopEntryID(e) == applied }) {
			next = applied
		} else if len(entries) > 0 {
			next = desktopEntryID(entries[0])
		}
		if next != "" {
			err = edit.SetJSON(p.meta, edit.KV{Path: "appliedId", Value: next})
		} else {
			err = edit.DelJSON(p.meta, "appliedId")
		}
		if err != nil {
			return err
		}
	}
	// the profile goes, and what is in it is kept for the next on: the
	// settings the user made in it are theirs (#2035)
	if b, err := os.ReadFile(p.prof); err == nil && desktopObjectBytes(b) && !desktopEmpty(b) {
		stash(map[string]string{desktopProfile + k: string(b)})
	}
	if err := os.Remove(p.prof); err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}

	// what magpie made, deepest first: a file only if it is an empty
	// object now, a folder only if it is empty
	for _, f := range slices.Backward(made) {
		st, err := os.Stat(f)
		if err != nil {
			continue
		}
		if st.IsDir() {
			os.Remove(f)
			continue
		}
		if b, _ := os.ReadFile(f); desktopEmpty(b) {
			os.Remove(f)
		}
	}
	return nil
}

// desktopOwnGatewayProfile is the profile the user made themselves in
// Desktop's window for magpie's gateway, the one applied before magpie
// applied its own: its settings (chat tab, default effort, allowed hosts
// and whatever Desktop adds) start magpie's profile, so switching on
// doesn't trade them for magpie's defaults (#2035). A profile for another
// gateway, Bedrock or Vertex is not one: its models and settings are for
// somewhere else. nil when there is none.
func desktopOwnGatewayProfile(p desktopPaths) []byte {
	id, _ := edit.GetJSON(p.meta, "appliedId")
	if id == desktopProfileID {
		id = unstash(desktopApplied + p.key)
	}
	if id == "" || id == desktopProfileID || filepath.Base(id) != id || strings.HasPrefix(id, ".") {
		return nil
	}
	b, err := os.ReadFile(filepath.Join(p.library, id+".json"))
	if err != nil || !desktopObjectBytes(b) {
		return nil
	}
	var prof struct {
		Provider string `json:"inferenceProvider"`
		BaseURL  string `json:"inferenceGatewayBaseUrl"`
	}
	if json.Unmarshal(b, &prof) != nil || prof.Provider != "gateway" || !desktopIsOurGateway(prof.BaseURL) {
		return nil
	}
	return b
}

// desktopIsOurGateway: u is magpie's gateway, by its port on this machine
// (127.0.0.1, localhost or ::1, with or without /v1).
func desktopIsOurGateway(u string) bool {
	a, err := url.Parse(strings.TrimSpace(u))
	if err != nil || a.Port() == "" {
		return false
	}
	ours, err := url.Parse(gateway.URL())
	if err != nil || a.Port() != ours.Port() {
		return false
	}
	h := a.Hostname()
	if h == ours.Hostname() || h == "localhost" {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

// desktopObjectBytes: b is a JSON object.
func desktopObjectBytes(b []byte) bool {
	var m map[string]json.RawMessage
	return json.Unmarshal(b, &m) == nil && m != nil
}

func desktopEmpty(b []byte) bool {
	var m map[string]json.RawMessage
	return json.Unmarshal(b, &m) == nil && len(m) == 0
}

// what claudedesktop says after a change (notice.go)
var (
	noticeClaudedesktop1 = newNotice("Claude Desktop reads this at start-up — quit and reopen it to run on magpie (Code and Cowork, no Anthropic sign-in).")
	noticeClaudedesktop2 = newNotice("Claude Desktop reads this at start-up — quit and reopen it to sign in with Anthropic again.")
)
