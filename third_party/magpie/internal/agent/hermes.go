package agent

// Hermes Agent (Nous Research) keeps its settings in $HERMES_HOME/config.yaml,
// by default %LOCALAPPDATA%\hermes\config.yaml on Windows and
// ~/.hermes/config.yaml elsewhere (place.hermesHome): the model under model.default, served by
// model.provider, which may name an entry of providers. magpie adds itself
// there as the provider "magpie" (the gateway, spoken to as chat completions,
// with the catalog as its models) and points model.provider at it; a model
// through magpie is "magpie/<provider>/<model>" here and model.default holds
// "<provider>/<model>". The provider and model the user had are stashed and
// put back when magpie steps out. A model of the user's own picked after
// that, in Hermes or on the Agents page, leaves magpie's provider in
// providers: Hermes stays connected, its session model picker still offers
// magpie's models beside its own (lijiho96940561 on X: Hermes went to Not
// connected and magpie's models left its picker), and only Disconnect
// takes the provider out.
//
// Each of Hermes's bots is a profile with its own config.yaml (see
// hermesProfiles), which doesn't read the main one's: magpie's provider goes
// into every profile's too, so magpie is offered in every bot's picker, not
// the main one's alone (lijiho96940561 on X). A profile's own model and
// provider are left as its user set them, and Disconnect takes the provider
// out of each.

import (
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/yetone/magpie/internal/dirent"
	"github.com/yetone/magpie/internal/edit"
)

func hermes(home string) *Agent { return hermesIn(here(home)) }

// hermesIn is Hermes at a place: this machine's home, or a WSL distro's
// (see wsl.go), where HERMES_HOME isn't read and its provider names the
// gateway as the distro reaches it.
func hermesIn(at place) *Agent {
	dir := at.hermesHome()
	path := filepath.Join(dir, "config.yaml")
	hermesProvider := func(p string) any { return hermesKept(p, hermesProviderAt(at.gw())) }
	// spread puts magpie's provider, as it is now, into every other
	// profile's config beside the one magpie connects
	spread := func() error {
		var errs []error
		for _, p := range hermesOthers(at, dir) {
			if edit.GetYAMLMap(p, "providers."+magpieID) != nil {
				errs = append(errs, syncYAML(p, "providers."+magpieID, func() any { return hermesProvider(p) }))
			} else {
				errs = append(errs, edit.SetYAML(p, edit.KV{Path: "providers." + magpieID, Value: hermesProvider(p)}))
			}
		}
		return errors.Join(errs...)
	}
	key := "hermes:" + path + ":"
	getKey := func(k string) string { v, _ := edit.GetYAML(path, k); return v }
	onMagpie := func() bool { return getKey("model.provider") == magpieID }
	// joined: magpie's provider is in providers, though the model is one of
	// Hermes's own
	joined := func() bool { _, ok := edit.GetYAML(path, "providers."+magpieID+".base_url"); return ok }
	// restore puts back the provider and model the user had before magpie,
	// magpie's provider left beside them
	restore := func() error {
		var del []string
		var kvs []edit.KV
		for _, k := range []string{"model.provider", "model.default"} {
			if v := unstash(key + k); v != "" {
				kvs = append(kvs, edit.KV{Path: k, Value: v})
			} else {
				del = append(del, k)
			}
		}
		if len(del) > 0 {
			if err := edit.DelYAML(path, del...); err != nil {
				return err
			}
		}
		if len(kvs) == 0 {
			return nil
		}
		return edit.SetYAML(path, kvs...)
	}
	return &Agent{
		ID: "hermes", Name: "Hermes Agent", Icon: "hermes", Aliases: []string{"hermes-agent"}, Spelled: prefixed, NoNames: true,
		UA:  []string{"hermes-agent"},
		Bin: "hermes", Dir: dir, Path: path,
		Sync: func() error {
			if err := syncYAML(path, "providers."+magpieID, func() any { return hermesProvider(path) }); err != nil || !joined() {
				return err
			}
			return spread()
		},
		Notice: func() string {
			if Running(`(^|/)hermes( |$)`) {
				return noticeHermes.String()
			}
			return ""
		},
		Joined: joined,
		// Disconnect takes magpie's provider out; the model, when it is
		// magpie's, goes back by the field
		Unwire: func() error {
			var errs []error
			if joined() {
				errs = append(errs, edit.DelYAML(path, "providers."+magpieID))
			}
			for _, p := range hermesOthers(at, dir) {
				errs = append(errs, hermesUnwireProfile(p))
			}
			return errors.Join(errs...)
		},
		Check: func() string {
			if !onMagpie() {
				return ""
			}
			return wiringOff("Hermes", path, func(k string) (string, bool) { return edit.GetYAML(path, "providers."+magpieID+"."+k) },
				"base_url", at.v1(), "api_key", at.gwKey())
		},
		Fields: []Field{{
			Key: "model", Label: "model",
			Get: func() string {
				v := getKey("model.default")
				if v == "" {
					v = getKey("model.model")
				}
				if v != "" && onMagpie() {
					return magpieID + "/" + v
				}
				return v
			},
			Set: func(v string) error {
				if v == "" {
					if onMagpie() {
						return restore()
					}
					return edit.DelYAML(path, "model.default")
				}
				if ref, ok := strings.CutPrefix(v, magpieID+"/"); ok && isMagpie(ref) {
					if !onMagpie() {
						stash(map[string]string{
							key + "model.provider": getKey("model.provider"),
							key + "model.default":  getKey("model.default"),
						})
					}
					if err := edit.SetYAML(path,
						edit.KV{Path: "providers." + magpieID, Value: hermesProvider(path)},
						edit.KV{Path: "model.provider", Value: magpieID},
						edit.KV{Path: "model.default", Value: ref},
					); err != nil {
						return err
					}
					return spread()
				}
				if onMagpie() {
					if err := restore(); err != nil {
						return err
					}
				}
				return edit.SetYAML(path, edit.KV{Path: "model.default", Value: v})
			},
			Options: func(cur map[string]string) []Option {
				return append(ownOptions("", cur["model"]), viaMagpie("hermes", magpieID+"/")...)
			},
		}, {
			// agent.reasoning_effort, which Hermes's /reasoning saves; none
			// turns reasoning off, and unset Hermes asks for medium
			Key: "effort", Label: "effort",
			Get: func() string { return getKey("agent.reasoning_effort") },
			Set: func(v string) error {
				if v == "" {
					return edit.DelYAML(path, "agent.reasoning_effort")
				}
				return edit.SetYAML(path, edit.KV{Path: "agent.reasoning_effort", Value: v})
			},
			Options: func(map[string]string) []Option {
				return static("none", "minimal", "low", "medium", "high", "xhigh")
			},
		}},
	}
}

// hermesProfileID is the name a Hermes profile may have
// (hermes_constants.PROFILE_ID_RE).
var hermesProfileID = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,63}$`)

// hermesProfiles is the config.yaml of each of Hermes's own profiles (its
// bots), as Hermes lists them (hermes_cli/profiles.py): the root's own, and
// <root>/profiles/<name>'s for each name Hermes allows that holds a file
// only a profile has and isn't deleted (a tombstone at
// profiles/.deleted/<name>). The root is Hermes's default folder
// (%LOCALAPPDATA%\hermes on Windows, ~/.hermes elsewhere), which a
// HERMES_HOME under it is a profile of; a HERMES_HOME elsewhere is the root itself, or, as
// <root>/profiles/<name>, a profile of <root> (get_default_hermes_root).
// A profile with no config.yaml yet is left out: magpie doesn't make one.
func hermesProfiles(at place) []string {
	root := at.hermesDefault()
	if env := at.getenv("HERMES_HOME"); env != "" {
		if rel, err := filepath.Rel(root, env); err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			root = env
			if filepath.Base(filepath.Dir(env)) == "profiles" {
				root = filepath.Dir(filepath.Dir(env))
			}
		}
	}
	paths := []string{filepath.Join(root, "config.yaml")}
	ps := filepath.Join(root, "profiles")
	es, _ := os.ReadDir(ps)
	for _, e := range es {
		name := e.Name()
		if !dirent.IsFolder(ps, e) || name == "default" || !hermesProfileID.MatchString(name) {
			continue
		}
		if _, err := os.Stat(filepath.Join(ps, ".deleted", name)); err == nil {
			continue
		}
		if st, err := os.Stat(filepath.Join(ps, name, "config.yaml")); err == nil && st.Mode().IsRegular() {
			paths = append(paths, filepath.Join(ps, name, "config.yaml"))
		}
	}
	return paths
}

// hermesOthers is every profile's config but the one at dir, which magpie
// connects
func hermesOthers(at place, dir string) []string {
	var out []string
	for _, p := range hermesProfiles(at) {
		if filepath.Clean(filepath.Dir(p)) == filepath.Clean(dir) {
			continue
		}
		if _, err := os.Stat(p); err != nil {
			continue
		}
		out = append(out, p)
	}
	return out
}

// hermesUnwireProfile takes magpie's provider out of a profile's config. A
// profile whose model is on magpie (one cloned from a connected main
// config, or picked in that bot) would be left naming a provider that is
// gone, so its provider and model go too, and Hermes takes its own
// defaults, as the main config does with nothing stashed.
func hermesUnwireProfile(p string) error {
	if edit.GetYAMLMap(p, "providers."+magpieID) == nil {
		return nil
	}
	del := []string{"providers." + magpieID}
	if v, _ := edit.GetYAML(p, "model.provider"); v == magpieID {
		del = append(del, "model.provider", "model.default")
	}
	return edit.DelYAML(p, del...)
}

type hermesProviderEntry struct {
	Name    string            `yaml:"name"`
	BaseURL string            `yaml:"base_url"`
	APIKey  string            `yaml:"api_key"`
	APIMode string            `yaml:"api_mode"`
	Headers map[string]string `yaml:"extra_headers"`
	Models  []string          `yaml:"models"`
}

// hermesProviderAt is magpie's entry under providers, for a Hermes reaching
// the gateway at gw. Hermes sends its own User-Agent only from a recent
// release on, so the header names it for the gateway's usage view.
func hermesProviderAt(gw string) hermesProviderEntry {
	ms := []string{}
	for _, m := range magpieModels("hermes") {
		ms = append(ms, m.ID)
	}
	return hermesProviderEntry{
		Name: magpieID, BaseURL: gw + "/v1", APIKey: keyAt(gw), APIMode: "chat_completions",
		Headers: map[string]string{"User-Agent": "hermes-agent"}, Models: ms,
	}
}

// hermesKept is magpie's entry e laid over the one in the config at path.
// Hermes keeps keys it doesn't know and writes into the entry itself: `hermes
// model` sets default_model there, the dashboard key_env, and a user adds
// key_cmd, request_timeout_seconds, extra_body, context_length, enabled or
// ssl_* (hermes_cli/config_providers.py _KNOWN_PROVIDER_KEYS). Each stays,
// as does a name of their own and a header beside magpie's User-Agent; the
// address, key, API mode and model list are magpie's (#2030).
func hermesKept(path string, e hermesProviderEntry) any {
	if name, ok := edit.GetYAML(path, "providers."+magpieID+".name"); ok && strings.TrimSpace(name) != "" {
		e.Name = name
	}
	return yamlKept(path, "providers."+magpieID, e, "base_url", "api_key", "api_mode", "models")
}

// what hermes says after a change (notice.go)
var (
	noticeHermes = newNotice("Hermes reads its settings at start-up — restart open Hermes sessions to use this.")
)
