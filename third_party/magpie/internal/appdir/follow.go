package appdir

import (
	"os"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/agentenv"
)

// An agent's variable (agentenv.Vars) on Windows is read as a process
// started now would get it, not as magpie was started with it. Windows
// gives a process its variables once, from the registry, and a User or
// System variable changed after that never reaches it: magpie runs for
// days in the tray, so a PI_CODING_AGENT_DIR the user moved kept naming
// the old folder, and magpie wrote Pi's models.json and settings.json into
// it again after the user had deleted it (bm_rabbit on Discord, 0.1.2405).
//
// Only a variable that came from the registry follows it: one whose value
// in magpie's environment at start was the registry's then (unset in both
// counts). One the launcher set itself (`$env:CODEX_HOME=…; magpie`, a
// test's sandbox) is kept, and so is one changed inside magpie since
// (os.Setenv, a test's t.Setenv), as it was before. A value followed is
// set in magpie's environment too, so the programs magpie starts (Pi,
// Codex's app-server, an agent's CLI) read the folder magpie writes.

// regEnv is a variable as the registry gives it to a process started now
// — the user's value, else the machine's, %…% expanded — and whether it
// has one; nil where there is no registry.
var regEnv = osRegEnv

// followFor is how long a registry read is used before it is read again:
// the variables are read on every request and listing.
var followFor = time.Second

type envVal struct {
	v  string
	ok bool
}

var follow struct {
	sync.Mutex
	vars map[string]*followed
}

// followed is one variable that follows the registry: the value magpie's
// environment holds while nothing in magpie has changed it, and the last
// registry read.
type followed struct {
	env envVal
	reg envVal
	at  time.Time
}

func init() { followStart() }

// followStart notes which agent variables came from the registry: those
// whose value in magpie's environment is the registry's now.
func followStart() {
	follow.Lock()
	defer follow.Unlock()
	follow.vars = nil
	if regEnv == nil {
		return
	}
	follow.vars = map[string]*followed{}
	now := time.Now()
	for _, name := range agentenv.Vars {
		v, ok := os.LookupEnv(name)
		r, rok := regEnv(name)
		if ok == rok && v == r {
			follow.vars[name] = &followed{env: envVal{v, ok}, reg: envVal{r, rok}, at: now}
		}
	}
}

// lookupEnv is os.LookupEnv, but an agent variable that came from the
// registry as it is there now (see above).
func lookupEnv(name string) (string, bool) {
	follow.Lock()
	defer follow.Unlock()
	f := follow.vars[name]
	v, ok := os.LookupEnv(name)
	if f == nil {
		return v, ok
	}
	if (envVal{v, ok}) != f.env {
		// changed inside magpie: what was set stands, from now on
		delete(follow.vars, name)
		return v, ok
	}
	if now := time.Now(); now.Sub(f.at) >= followFor || now.Before(f.at) {
		r, rok := regEnv(name)
		f.reg, f.at = envVal{r, rok}, now
	}
	if f.reg != f.env {
		if f.reg.ok {
			os.Setenv(name, f.reg.v)
		} else {
			os.Unsetenv(name)
		}
		f.env = f.reg
	}
	return f.env.v, f.env.ok
}
