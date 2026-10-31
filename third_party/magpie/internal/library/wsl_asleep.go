package library

import (
	"encoding/json"
	"log"
	"maps"
	"slices"
	"strings"
	"sync"

	"github.com/yetone/magpie/internal/agent"
)

// A stopped WSL distro's agents are no targets (wslTargetOf): their files
// can't be opened without starting the distro (#3896). The page lists them
// all the same (cybercaius on Discord: the MCP page knew a distro's agents
// only once WSL had started), from what magpie learnt of them while the
// distro ran — the places it gives each one MCP servers and skills, kept
// in wsl.json beside the probe's memory (shape) — with the ticks the
// library has for them. A tick there is kept as any is, in library.json,
// and written into the distro the next time it runs (Woken): nothing is
// opened while it is stopped, and nothing starts it but the user.

// shape is what the library last knew of where a WSL agent keeps its MCP
// servers and skills, and what its MCP file can take: an AgentView's, so
// the page can show the agent's ticks while its distro is stopped.
type shape struct {
	MCP        string   `json:"mcp,omitempty"`
	Skills     string   `json:"skills,omitempty"`
	SkillsAlso []string `json:"skillsAlso,omitempty"`
	MCPVia     string   `json:"mcpVia,omitempty"`
	NoSSE      bool     `json:"noSSE,omitempty"`
	NoRemote   bool     `json:"noRemote,omitempty"`
	NoEnvRefs  bool     `json:"noEnvRefs,omitempty"`
	ToolsOff   bool     `json:"toolsOff,omitempty"`
	Startup    bool     `json:"startup,omitempty"`
}

// shapeKey is the key a WSL agent's shape is kept under in its distro's
// memory: one for each kind of agent (codex, omp#work).
func shapeKey(id string) string {
	kind, _, _ := strings.Cut(id, "@")
	return "library:" + kind
}

// keepShape remembers t's shape, t being an agent in a running distro.
func keepShape(t *Target) {
	a := t.Agent
	sh := shape{Skills: t.Skills, SkillsAlso: t.SkillsAlso, MCPVia: t.MCPVia}
	if t.MCP != nil {
		sh.MCP = t.MCP.Path
		sh.NoSSE = t.MCP.supports(&Server{Transport: "sse"}) != nil
		sh.NoRemote = t.MCP.supports(&Server{Transport: "http"}) != nil
		sh.NoEnvRefs = t.MCP.refsOf() == envSyntax{}
		sh.ToolsOff = toolsOffOf(t) != toolsNone
		sh.Startup = t.MCP.Format == fmtCodex
	}
	b, err := json.Marshal(sh)
	if err != nil {
		return
	}
	if string(b) != agent.WSLKept(a.WSL, shapeKey(a.ID)) {
		agent.WSLKeep(a.WSL, shapeKey(a.ID), string(b))
	}
}

// shapeOf is the shape kept for a, an agent in a stopped distro; nil when
// magpie never saw it with an MCP file or a skills folder.
func shapeOf(a *agent.Agent) *shape {
	var sh shape
	if json.Unmarshal([]byte(agent.WSLKept(a.WSL, shapeKey(a.ID))), &sh) != nil || sh.MCP == "" && sh.Skills == "" {
		return nil
	}
	return &sh
}

// reaches says whether the agent can be given s, as far as its shape
// tells (the page's reaches): what its file was found to read when the
// distro last ran. The rest (a header Codex reads only whole) is said by
// the sync that writes it, as for any agent.
func (sh *shape) reaches(s *Server) bool {
	switch {
	case sh.MCP == "":
		return false
	case s.Transport == "sse" && sh.NoSSE, s.Remote() && sh.NoRemote:
		return false
	case sh.NoEnvRefs && (s.Remote() && hasRef(s.Headers) || !s.Remote() && hasRef(s.Env)):
		return false
	case !s.Remote() && windowsPath.MatchString(s.Command) && !isSelf(s):
		return false
	}
	return true
}

// isAsleep says whether a is an agent in a WSL distro that isn't running
// now: wslTargetOf's nil.
func isAsleep(a *agent.Agent) bool {
	return a.WSL != "" && (a.Home == "" || !agent.WSLRunning(a.WSL))
}

// asleepView is a stopped distro's agent as the page lists it: by its
// shape, or, with none known and the library naming it, as one whose
// ticks can't be shown (Unseen); false for one the page leaves out.
func (l *Library) asleepView(a *agent.Agent) (AgentView, bool) {
	av := AgentView{ID: a.ID, Name: a.Name, Icon: a.Icon, Asleep: a.WSL}
	sh := shapeOf(a)
	if sh == nil {
		av.Unseen = true
		return av, l.names(a.ID)
	}
	av.MCP, av.Skills, av.SkillsAlso, av.MCPVia = sh.MCP, sh.Skills, sh.SkillsAlso, sh.MCPVia
	av.NoSSE, av.NoRemote, av.NoEnvRefs, av.ToolsOff, av.Startup = sh.NoSSE, sh.NoRemote, sh.NoEnvRefs, sh.ToolsOff, sh.Startup
	if sh.Skills != "" {
		// a WSL agent takes copies only (Target.Copy)
		_, av.HowOwn = l.SkillHow[a.ID]
		av.How, av.MustCopy = HowCopy, true
	}
	return av, true
}

// names says whether the library gives the agent id anything, or holds
// something it gave it.
func (l *Library) names(id string) bool {
	if a := l.Applied[id]; a != nil && (len(a.Skills) > 0 || len(a.MCP) > 0 || len(a.Hooks) > 0) {
		return true
	}
	return slices.ContainsFunc(l.Skills, func(s *Skill) bool { return slices.Contains(s.Agents, id) }) ||
		slices.ContainsFunc(l.MCP, func(s *Server) bool { return slices.Contains(s.Agents, id) })
}

// distrosNamed are the WSL distros the library names an agent in.
func (l *Library) distrosNamed() []string {
	in := map[string]bool{}
	add := func(id string) {
		if _, d, ok := strings.Cut(id, "@wsl:"); ok && d != "" {
			in[d] = true
		}
	}
	for id := range l.Applied {
		if l.names(id) {
			add(id)
		}
	}
	for _, s := range l.Skills {
		for _, id := range s.Agents {
			add(id)
		}
	}
	for _, s := range l.MCP {
		for _, id := range s.Agents {
			add(id)
		}
	}
	return slices.Sorted(maps.Keys(in))
}

// wake is which distros were last seen stopped (false) or running (true),
// by Woken and by the reads that left a stopped one's agents out.
var wake struct {
	sync.Mutex
	up map[string]bool
}

func noteUp(distro string, up bool) {
	wake.Lock()
	defer wake.Unlock()
	if wake.up == nil {
		wake.up = map[string]bool{}
	}
	wake.up[distro] = up
}

// Woken writes the library into the agents of each distro it names that
// was seen stopped and runs now: what was ticked for them while it was
// stopped, kept in library.json, reaches them once the user starts it.
// Asking whether a distro runs (wsl.exe -l --running) doesn't start it.
// nil, nil when no such distro started.
func Woken() (*Result, error) {
	mu.Lock()
	l, err := load()
	mu.Unlock()
	if err != nil {
		return nil, err
	}
	started := false
	for _, d := range l.distrosNamed() {
		up := agent.WSLRunning(d)
		wake.Lock()
		was, seen := wake.up[d]
		wake.Unlock()
		noteUp(d, up)
		if up && seen && !was {
			started = true
		}
	}
	if !started {
		return nil, nil
	}
	return Sync()
}

// keepWoken is Woken for KeepLinked's loop, which logs what it couldn't do.
func keepWoken() {
	res, err := Woken()
	if err != nil {
		log.Println("library: a WSL distro that started:", err)
		return
	}
	if res != nil {
		for _, p := range res.Problems {
			log.Println("library: a WSL distro that started:", p.Agent, p.What, p.Error)
		}
	}
}

// AsleepIn is the WSL distro, not running now, of the agent with id; ""
// for any other. A change the library makes for it is written there the
// next time it runs.
func AsleepIn(id string) string {
	_, d, ok := strings.Cut(id, "@wsl:")
	if !ok || d == "" || agent.WSLRunning(d) {
		return ""
	}
	return d
}
