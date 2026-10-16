package pluginrt

import (
	"context"
	"errors"
	"fmt"
	"log"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"gopkg.in/yaml.v3"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/filelock"
)

// plugins.yml says which built-in plugins are on, and each one's config:
//
//	preset: essentials   # or all
//	plugins:
//	  - id: groups
//	    enabled: true
//	    config: {}
//
// What a plugin runs with is composed, field by field, from three layers:
// the built-in default (every plugin on, its Manifest.Defaults), the
// preset (Essentials leaves the plugins not Essential off), then the
// user's own rows. A plugin in use (Manifest.InUse) is on whatever they
// say. `magpie plugin dump` shows the result and where each value came
// from.
//
// A new install is written the Essentials preset, a magpie that ran here
// before All (Settle), so an existing user keeps everything they had.

// Presets a plugins.yml can name.
const (
	PresetEssentials = "essentials"
	PresetAll        = "all"
)

// File is plugins.yml as written.
type File struct {
	Preset  string `yaml:"preset,omitempty"`
	Plugins []Row  `yaml:"plugins,omitempty"`
}

// Row is the user's own say about one plugin: only what they set.
type Row struct {
	ID      string         `yaml:"id"`
	Enabled *bool          `yaml:"enabled,omitempty"`
	Config  map[string]any `yaml:"config,omitempty"`
}

// Path is plugins.yml, in magpie's folder.
func Path() string { return filepath.Join(appdir.Config(), "plugins.yml") }

var (
	inProcess sync.Mutex // filelock.Mutex holds other magpies off; this, this one's goroutines
	fileLock  = filelock.Mutex{Path: func() string { return Path() + ".lock" }}
)

// Load reads plugins.yml. A file not there is (File{}, false, nil): the
// default layers alone. One that can't be read or doesn't parse is an
// error, never an empty file.
func Load() (File, bool, error) {
	b, err := os.ReadFile(Path())
	if errors.Is(err, os.ErrNotExist) {
		return File{}, false, nil
	}
	if err != nil {
		return File{}, true, err
	}
	var f File
	if err := yaml.Unmarshal(b, &f); err != nil {
		return File{}, true, fmt.Errorf("%s doesn't parse: %w", Path(), err)
	}
	switch f.Preset {
	case "", PresetAll, PresetEssentials:
	default:
		return File{}, true, fmt.Errorf("%s: no preset %q (essentials or all)", Path(), f.Preset)
	}
	return f, true, nil
}

// Source says which layer a composed value came from.
type Source string

const (
	FromDefault Source = "default"
	FromPreset  Source = "preset"
	FromUser    Source = "user"
	FromInUse   Source = "in use"
)

// Composed is one plugin as plugins.yml and its use make it.
type Composed struct {
	ID      string            `json:"id" yaml:"id"`
	Enabled bool              `json:"enabled" yaml:"enabled"`
	Source  Source            `json:"source" yaml:"source"`
	Chosen  bool              `json:"chosen" yaml:"chosen"` // plugins.yml's say, before what is in use
	InUse   int               `json:"inUse" yaml:"inUse,omitempty"`
	Config  map[string]any    `json:"config,omitempty" yaml:"config,omitempty"`
	Sources map[string]Source `json:"configSources,omitempty" yaml:"configSources,omitempty"`
}

// Compose layers f over the defaults for each manifest, in ms' order.
func Compose(ms []Manifest, f File) []Composed {
	rows := map[string]Row{}
	for _, r := range f.Plugins {
		rows[r.ID] = r
	}
	out := make([]Composed, 0, len(ms))
	for _, m := range ms {
		c := Composed{ID: m.ID, Enabled: true, Source: FromDefault, Config: map[string]any{}, Sources: map[string]Source{}}
		for k, v := range m.Defaults {
			c.Config[k], c.Sources[k] = v, FromDefault
		}
		if f.Preset == PresetEssentials && !m.Essential {
			c.Enabled, c.Source = false, FromPreset
		}
		if r, ok := rows[m.ID]; ok {
			if r.Enabled != nil {
				c.Enabled, c.Source = *r.Enabled, FromUser
			}
			for k, v := range r.Config {
				// a value its settings don't allow (plugins.yml edited by
				// hand) is left out, the one under it kept; the file keeps it
				if s := m.Schema(); s != nil {
					ok, err := s.Validate(map[string]any{k: v})
					if err != nil {
						log.Printf("plugins: %s's %s in %s: %v", m.ID, k, Path(), err)
						continue
					}
					v = ok[k]
				}
				c.Config[k], c.Sources[k] = merge(c.Config[k], v), FromUser
			}
		}
		c.Chosen = c.Enabled
		if m.InUse != nil {
			n, err := m.InUse()
			if err != nil {
				n = 1 // can't be told: unknown is never unused
			}
			c.InUse = n
			if n > 0 && !c.Enabled {
				c.Enabled, c.Source = true, FromInUse
			}
		}
		if len(c.Config) == 0 {
			c.Config, c.Sources = nil, nil
		}
		out = append(out, c)
	}
	return out
}

// merge lays the user's value over the one under it: a map key by key,
// anything else whole.
func merge(under, over any) any {
	u, ok1 := under.(map[string]any)
	o, ok2 := over.(map[string]any)
	if !ok1 || !ok2 {
		return over
	}
	out := maps.Clone(u)
	for k, v := range o {
		out[k] = merge(out[k], v)
	}
	return out
}

// Settle writes a first plugins.yml when there is none: the Essentials
// preset on a new install, All on a magpie that ran here before. It says
// whether it wrote one. A plugins.yml there, or one that can't be looked
// at, is left as it is.
func Settle(newInstall bool) (bool, error) {
	inProcess.Lock()
	defer inProcess.Unlock()
	fileLock.Lock()
	defer fileLock.Unlock()
	if _, err := os.Lstat(Path()); !errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	f := File{Preset: PresetAll}
	if newInstall {
		f.Preset = PresetEssentials
	}
	return true, write(f)
}

// Set turns plugin id on or off as the user's own choice, keeping the rest
// of plugins.yml as it was. A plugins.yml that doesn't parse is not
// written over: the user fixes it, or removes it.
func Set(id string, on bool) error {
	return update(func(f *File) {
		i := slices.IndexFunc(f.Plugins, func(r Row) bool { return r.ID == id })
		if i < 0 {
			f.Plugins = append(f.Plugins, Row{ID: id})
			i = len(f.Plugins) - 1
		}
		f.Plugins[i].Enabled = &on
	})
}

// SetConfig sets plugin id's settings in config as the user's own,
// keeping its others and the rest of plugins.yml as they were. Only what
// its settings' schema allows is taken: a key it doesn't have, or a value
// of the wrong kind or out of range, is refused and nothing is written.
func SetConfig(id string, config map[string]any) error {
	var s *Schema
	for _, m := range Registered() {
		if m.ID == id {
			if s = m.Schema(); s == nil {
				return fmt.Errorf("%s has no settings", id)
			}
		}
	}
	if s == nil {
		return fmt.Errorf("no built-in plugin %q", id)
	}
	ok, err := s.Validate(config)
	if err != nil {
		return err
	}
	return update(func(f *File) {
		i := slices.IndexFunc(f.Plugins, func(r Row) bool { return r.ID == id })
		if i < 0 {
			f.Plugins = append(f.Plugins, Row{ID: id})
			i = len(f.Plugins) - 1
		}
		if f.Plugins[i].Config == nil {
			f.Plugins[i].Config = map[string]any{}
		}
		maps.Copy(f.Plugins[i].Config, ok)
	})
}

// SetPreset changes the preset, keeping the user's own rows.
func SetPreset(preset string) error {
	if preset != PresetAll && preset != PresetEssentials {
		return fmt.Errorf("no preset %q (essentials or all)", preset)
	}
	return update(func(f *File) { f.Preset = preset })
}

func update(change func(*File)) error {
	inProcess.Lock()
	defer inProcess.Unlock()
	fileLock.Lock()
	defer fileLock.Unlock()
	f, _, err := Load()
	if err != nil {
		return err
	}
	change(&f)
	return write(f)
}

const header = "# magpie's built-in plugins: which are on, and each one's config.\n" +
	"# `magpie plugin dump` shows what they come to, and where each value is from.\n"

// write puts f in plugins.yml whole or not at all, the file it replaces
// kept as plugins.yml.bak. Called with the locks held.
func write(f File) error {
	b, err := yaml.Marshal(f)
	if err != nil {
		return err
	}
	b = append([]byte(header), b...)
	path := Path()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	if old, err := os.ReadFile(path); err == nil {
		if err := os.WriteFile(path+".bak", old, 0o600); err != nil {
			return fmt.Errorf("keeping %s before writing it: %w", path, err)
		}
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".plugins.yml-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

var (
	defMu sync.Mutex
	def   *Runtime
)

// Refresh composes plugins.yml with the registered plugins and brings the
// process's runtime to it, made on the first call. A plugins.yml that
// can't be read leaves the runtime as it was (every plugin on, before the
// first good read) and is returned.
func Refresh() ([]Composed, error) {
	defMu.Lock()
	defer defMu.Unlock()
	ms := Registered()
	if def == nil {
		def = New(ms)
	}
	f, _, err := Load()
	if err != nil {
		if !def.reconciled {
			def.Reconcile(allOn(ms), nil)
		}
		return nil, err
	}
	cs := Compose(ms, f)
	on, config := map[string]bool{}, map[string]map[string]any{}
	for _, c := range cs {
		on[c.ID], config[c.ID] = c.Enabled, c.Config
	}
	def.Reconcile(on, config)
	return cs, nil
}

// KeepFresh refreshes the process's runtime (Refresh) as it starts and
// whenever plugins.yml, or one of also (a file an InUse reads), changes,
// until ctx ends. The process serving the gateway runs it: the window or
// `magpie plugin builtin` in another process may turn a plugin on or off,
// and what a plugin registers with the gateway (an observer) follows
// without a restart.
func KeepFresh(ctx context.Context, every time.Duration, also ...string) {
	files := append([]string{Path()}, also...)
	stamp := func() string {
		var b strings.Builder
		for _, p := range files {
			if fi, err := os.Stat(p); err == nil {
				fmt.Fprintf(&b, "%d/%d;", fi.ModTime().UnixNano(), fi.Size())
			} else {
				b.WriteString("-;")
			}
		}
		return b.String()
	}
	last := ""
	tick := time.NewTicker(every)
	defer tick.Stop()
	for {
		if now := stamp(); now != last {
			last = now
			if _, err := Refresh(); err != nil {
				log.Printf("plugins: %v", err)
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

func allOn(ms []Manifest) map[string]bool {
	on := map[string]bool{}
	for _, m := range ms {
		on[m.ID] = true
	}
	return on
}

// Running is the process's runtime, nil before the first Refresh.
func Running() *Runtime {
	defMu.Lock()
	defer defMu.Unlock()
	return def
}
