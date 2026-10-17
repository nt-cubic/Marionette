package library

import (
	"bytes"
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/yetone/magpie/internal/appdir"
)

// Where a skill came from, for the page to group it with the others from
// the same repository (White Immortal on Discord: the skills of
// github.com/mattpocock/skills together, however each came in). One
// installed from GitHub says so itself; for the others it is traced:
// CC Switch's record, the skills CLI's lock (npx skills add, in
// ~/.agents/.skill-lock.json or $XDG_STATE_HOME/skills), the git checkout a linked folder is in,
// or the repository the user installed from since that has the very same
// skill (From). It only groups: such a skill is not updated from there,
// unless the library's own copy is the repository's to the file, which
// then is the repository's skill (noteFrom).

// repoOf is the repository a library skill came from: owner/repo on
// GitHub, host/path elsewhere, "" when it can't be told.
func (tr *tracer) repoOf(s *Skill) string {
	if s.Source != nil && s.Source.Kind == "github" {
		return s.Source.Repo
	}
	if o, ok := ccSwitchOrigin(s); ok {
		return o.Repo
	}
	if s.From != "" {
		return fromKey(s.From)
	}
	dir := skillDir(s.Name)
	if !linked(dir) {
		// the library's own copy: the skills CLI's, moved in, by its name
		if e, ok := tr.lock()[s.Name]; ok && (s.Source == nil || within(realDir(s.Source.Dir), realDir(sharedSkillsDir()))) {
			return e
		}
		return ""
	}
	return tr.folder(realDir(dir))
}

// folder is the repository a skill's folder elsewhere came from: the
// skills CLI's lock for one in ~/.agents/skills, else its git checkout.
func (tr *tracer) folder(dir string) string {
	if within(dir, realDir(sharedSkillsDir())) {
		if e, ok := tr.lock()[filepath.Base(dir)]; ok {
			return e
		}
	}
	return gitRepoOf(dir)
}

// tracer reads what tracing needs once for a whole page.
type tracer struct {
	locked map[string]string
	read   bool
}

// lock is the skills CLI's lock: each skill's repository by its name.
// The CLI keeps it in ~/.agents, or under $XDG_STATE_HOME when that is set
// (skills 1.7: $XDG_STATE_HOME/skills/.skill-lock.json, the skills still
// in ~/.agents/skills). Both are read, the XDG one's entries winning: one
// written before the variable was set names skills the newer doesn't.
func (tr *tracer) lock() map[string]string {
	if !tr.read {
		tr.read, tr.locked = true, readSkillLock(filepath.Join(home(), ".agents", ".skill-lock.json"))
		if x := appdir.Getenv("XDG_STATE_HOME"); x != "" {
			for name, r := range readSkillLock(filepath.Join(x, "skills", ".skill-lock.json")) {
				if tr.locked == nil {
					tr.locked = map[string]string{}
				}
				tr.locked[name] = r
			}
		}
	}
	return tr.locked
}

func readSkillLock(path string) map[string]string {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var f struct {
		Skills map[string]struct {
			Source        string `json:"source"`
			SourceType    string `json:"sourceType"`
			SourceURL     string `json:"sourceUrl"`
			SourceBaseURL string `json:"sourceBaseUrl"`
		} `json:"skills"`
	}
	if json.Unmarshal(b, &f) != nil {
		return nil
	}
	out := map[string]string{}
	for name, e := range f.Skills {
		r := ""
		if e.SourceType == "github" && repoRe.MatchString(e.Source) {
			r = strings.TrimSuffix(e.Source, ".git")
		} else if e.SourceType == "well-known" {
			r = wellKnownBase(e.SourceBaseURL, e.SourceURL)
		} else if e.SourceURL != "" {
			r = remoteRepo(e.SourceURL)
		}
		if r != "" {
			out[name] = r
		}
	}
	return out
}

// wellKnownBase is where a kit of skills a site serves at its
// /.well-known/agent-skills/ came from (#1862, WardLu): the lock's
// sourceBaseUrl, as host/path, so Feishu's lark-base, lark-doc and
// lark-sheets are one group and Fish Audio's two another, while two kits
// under one host with different base URLs stay apart. Each skill's
// sourceUrl is its own file there, so it isn't the group. A lock written
// before sourceBaseUrl has the base in the sourceUrl, before its
// /.well-known/; one with neither is grouped by its sourceUrl as before.
func wellKnownBase(base, src string) string {
	if b := siteOf(base); b != "" {
		return b
	}
	if i := strings.Index(src, "/.well-known/"); i > 0 {
		if b := siteOf(src[:i]); b != "" {
			return b
		}
	}
	return remoteRepo(src)
}

// fromKey is the group a skill's From names. A skill brought in from
// ~/.agents/skills keeps the lock's key as it was then (ImportSkills), and
// before well-known kits were grouped by their base that was each skill's
// own sourceUrl, "docs.fish.audio/.well-known/agent-skills/fish-audio-api/
// skill.md": read as the base before /.well-known/, so a kit brought in
// then is one group as the lock now says (#1862, WardLu on v0.1.1223).
func fromKey(from string) string {
	if before, _, ok := strings.Cut(from, "/.well-known/"); ok && before != "" {
		return before
	}
	return from
}

// siteOf reads a web address as host/path, or the host alone, with no
// user, query or trailing slash: "" when it isn't an http(s) address.
func siteOf(s string) string {
	u, err := url.Parse(strings.TrimSpace(s))
	if err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Hostname() == "" {
		return ""
	}
	host := strings.TrimPrefix(strings.ToLower(u.Hostname()), "www.")
	p := strings.Trim(u.Path, "/")
	if before, _, ok := strings.Cut("/"+p+"/", "/.well-known/"); ok {
		p = strings.Trim(before, "/")
	}
	if strings.Contains(p, "..") {
		return ""
	}
	if p == "" {
		return host
	}
	return host + "/" + p
}

// gitRepoOf is the repository of the git checkout a skill's folder is, or
// is in a few folders down: its remote origin. A checkout that is the home
// folder, or a folder of an app's own in it (~/.claude kept in git), is
// the user's dotfiles rather than where the skill came from.
func gitRepoOf(dir string) string {
	h := realDir(home())
	for i := 0; i < 4 && dir != "" && dir != h; i++ {
		if p := filepath.Dir(dir); p == h && strings.HasPrefix(filepath.Base(dir), ".") {
			return ""
		}
		if fi, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			cfg := filepath.Join(dir, ".git", "config")
			if !fi.IsDir() {
				cfg = gitdirConfig(dir)
			}
			return originURL(cfg)
		}
		next := filepath.Dir(dir)
		if next == dir {
			break
		}
		dir = next
	}
	return ""
}

// gitdirConfig is the config of a checkout whose .git is a file (a
// worktree or a submodule): the folder it names, or its common one.
func gitdirConfig(dir string) string {
	b, err := os.ReadFile(filepath.Join(dir, ".git"))
	if err != nil {
		return ""
	}
	g, ok := strings.CutPrefix(strings.TrimSpace(string(b)), "gitdir:")
	if !ok {
		return ""
	}
	g = strings.TrimSpace(g)
	if !filepath.IsAbs(g) {
		g = filepath.Join(dir, g)
	}
	if c, err := os.ReadFile(filepath.Join(g, "commondir")); err == nil {
		cd := strings.TrimSpace(string(c))
		if !filepath.IsAbs(cd) {
			cd = filepath.Join(g, cd)
		}
		g = cd
	}
	return filepath.Join(g, "config")
}

var originSection = regexp.MustCompile(`^\[\s*remote\s+"origin"\s*\]$`)

// originURL is the repository remote "origin" of a git config fetches.
func originURL(cfg string) string {
	b, err := os.ReadFile(cfg)
	if err != nil {
		return ""
	}
	in := false
	for _, line := range bytes.Split(b, []byte("\n")) {
		l := strings.TrimSpace(string(line))
		if strings.HasPrefix(l, "[") {
			in = originSection.MatchString(l)
			continue
		}
		if k, v, ok := strings.Cut(l, "="); in && ok && strings.TrimSpace(k) == "url" {
			return remoteRepo(strings.TrimSpace(v))
		}
	}
	return ""
}

var scpRemote = regexp.MustCompile(`^(?:[\w.-]+@)?([\w.-]+\.[\w.-]+):(.+)$`)

// remoteRepo reads a git remote's address as a repository: owner/repo on
// GitHub, host/path elsewhere. A user or a token in it is never kept.
func remoteRepo(s string) string {
	s = strings.TrimSpace(s)
	host, p := "", ""
	if u, err := url.Parse(s); err == nil && u.Scheme != "" && u.Host != "" {
		host, p = u.Hostname(), u.Path
	} else if m := scpRemote.FindStringSubmatch(s); m != nil {
		host, p = m[1], m[2]
	} else if repoRe.MatchString(s) {
		host, p = "github.com", s
	} else {
		return ""
	}
	host = strings.TrimPrefix(strings.ToLower(host), "www.")
	p = strings.TrimSuffix(strings.Trim(p, "/"), ".git")
	if strings.Count(p, "/") < 1 || strings.Contains(p, "..") {
		return ""
	}
	if host == "github.com" {
		parts := strings.Split(p, "/")
		return parts[0] + "/" + parts[1]
	}
	return host + "/" + p
}

// noteFrom writes down, for the skills the library has already that a
// repository being installed from has too, that they came from there (and
// binds a copy that is the repository's whole to it, to update from):
// one whose SKILL.md is that repository's very own, and one traced to
// nowhere by the same name when the library has others from there. A
// pack added a while ago is mostly older copies (White Immortal on
// Discord: 20 of mattpocock/skills' 38 grouped, the rest edited there
// since), and a name the repository has beside others the user took
// from it is that skill. root is where the repository was fetched to. It
// gives how many it noted.
func noteFrom(src Source, root string, cands []Candidate) int {
	if src.Kind != "github" {
		return 0
	}
	mu.Lock()
	defer mu.Unlock()
	l, err := load()
	if err != nil {
		return 0
	}
	tr := &tracer{}
	have := slices.ContainsFunc(l.Skills, func(s *Skill) bool { return strings.EqualFold(tr.repoOf(s), src.Repo) })
	noted, bound := 0, 0
	var same []*Skill
	for _, c := range cands {
		s := l.skill(c.Name)
		if s == nil || (s.Source != nil && s.Source.Kind == "github") {
			continue
		}
		grouped := strings.EqualFold(s.From, src.Repo)
		mine, err1 := os.ReadFile(filepath.Join(skillDir(s.Name), "SKILL.md"))
		theirs, err2 := os.ReadFile(filepath.Join(root, filepath.FromSlash(c.Path), "SKILL.md"))
		if err1 != nil || err2 != nil {
			continue
		}
		if bytes.Equal(bytes.TrimSpace(mine), bytes.TrimSpace(theirs)) {
			if !grouped {
				s.From, have = src.Repo, true
				noted++
			}
			// the library's own copy, every file of it the repository's:
			// it is that skill, and updates from there as one installed
			// from it does (RockyShowlle on X). One linked to a folder
			// elsewhere is kept by whatever put it there (npx skills, a
			// git checkout), and an edited one stays only grouped.
			if dir := skillDir(s.Name); s.Source == nil && !linked(dir) && sameTree(dir, filepath.Join(root, filepath.FromSlash(c.Path))) {
				s.Source = &Source{Kind: "github", Repo: src.Repo, Ref: src.Ref, Path: c.Path}
				s.Hash, s.Commit, s.From = hashDir(dir), "", ""
				l.seenAt(src.Repo, src.Ref, c.Path)
				forgetCheck(s.Name)
				bound++
			}
		} else if !grouped && tr.repoOf(s) == "" {
			same = append(same, s)
		}
	}
	if have {
		for _, s := range same {
			s.From = src.Repo
			noted++
		}
	}
	if noted > 0 || bound > 0 {
		l.save()
	}
	return noted
}

// RegroupSkills traces the library's skills again (the Library page's
// Group again): each GitHub repository a skill is traced to is fetched,
// and the skills it has that aren't grouped with it yet are noted as from
// there, as installing from it again would. It gives how many were
// grouped; an error only when no repository could be fetched.
func RegroupSkills() (int, error) {
	l, err := load()
	if err != nil {
		return 0, err
	}
	tr := &tracer{}
	var repos []string
	seen := map[string]bool{}
	for _, s := range l.Skills {
		r := tr.repoOf(s)
		// a site's kit (docs.example.com/kit) reads like owner/repo, but
		// a GitHub owner has no dot
		if !repoRe.MatchString(r) || strings.Contains(strings.Split(r, "/")[0], ".") || seen[strings.ToLower(r)] {
			continue
		}
		seen[strings.ToLower(r)] = true
		repos = append(repos, r)
	}
	noted, fetched := 0, 0
	var first error
	for _, r := range repos {
		src := Source{Kind: "github", Repo: r}
		root, err := fetch(src)
		if err != nil {
			if first == nil {
				first = err
			}
			continue
		}
		fetched++
		noted += noteFrom(src, root, candidates(root, ""))
		os.RemoveAll(root)
	}
	if fetched == 0 && first != nil {
		return 0, first
	}
	return noted, nil
}
