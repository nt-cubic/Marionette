package plugin

// A plugin from a git repository has no npm version to compare: what
// tells it is out of date is its repository's commit. Bun records the
// commit it installed (node_modules/<pkg>/.bun-tag, and bun.lock); the
// repository is asked which commit its branch or tag is at now: GitHub's
// API for a repository there (bun fetches those as GitHub's tarballs, so
// no git is needed), git ls-remote for any other. An update is offered
// only when the two differ (ARNO on Discord: a plugin added as
// github:yuweihao17/codearts showed Update all the time, and still did
// right after updating it).

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"
)

var hexSHA = regexp.MustCompile(`^[0-9a-f]{7,40}$`)

// GitCommit is the commit bun installed of the git plugin spec, as bun
// recorded it (7 hex digits for a GitHub one, 40 for one fetched with
// git); "" for one that isn't git, or isn't installed.
func GitCommit(spec string) string {
	if !IsGit(spec) {
		return ""
	}
	name := Name(spec)
	if name == spec {
		return ""
	}
	// what bun wrote beside the package it installed: owner-repo-<sha>
	// for GitHub's, <sha> for one it cloned
	if b, err := os.ReadFile(filepath.Join(Target(spec), ".bun-tag")); err == nil {
		tag := strings.TrimSpace(string(b))
		if c := tag[strings.LastIndex(tag, "-")+1:]; hexSHA.MatchString(c) {
			return c
		}
	}
	// else bun.lock's: "<name>": ["<name>@<resolved>#<sha>", …]
	if b, err := os.ReadFile(filepath.Join(Dir(), "bun.lock")); err == nil {
		re := regexp.MustCompile(`"` + regexp.QuoteMeta(name) + `":\s*\["` + regexp.QuoteMeta(name) + `@[^"#]*#([0-9a-f]{7,40})"`)
		if m := re.FindSubmatch(b); m != nil {
			return string(m[1])
		}
	}
	return ""
}

// SameCommit is whether two commits are one: an abbreviated sha names the
// full one it begins.
func SameCommit(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	if len(a) > len(b) {
		a, b = b, a
	}
	return strings.HasPrefix(b, a)
}

// gitSource is where a git spec's repository is asked for its commits:
// owner/repo on GitHub, else the URL git ls-remote takes; and the branch,
// tag or commit after #, "" for the default branch.
func gitSource(spec string) (gh, remote, ref string) {
	s, ref, _ := strings.Cut(spec, "#")
	switch {
	case strings.HasPrefix(s, "github:"):
		gh = strings.TrimPrefix(s, "github:")
	case strings.HasPrefix(s, "gitlab:"):
		remote = "https://gitlab.com/" + strings.TrimPrefix(s, "gitlab:")
	case strings.HasPrefix(s, "bitbucket:"):
		remote = "https://bitbucket.org/" + strings.TrimPrefix(s, "bitbucket:")
	case !strings.Contains(s, ":"):
		gh = s // owner/repo, GitHub's shorthand
	default:
		remote = strings.TrimPrefix(s, "git+")
		if u, err := url.Parse(remote); err == nil && strings.TrimPrefix(strings.ToLower(u.Hostname()), "www.") == "github.com" && u.Scheme != "file" {
			gh = strings.TrimPrefix(u.Path, "/")
		}
	}
	if gh != "" {
		gh = strings.TrimSuffix(strings.Trim(gh, "/"), ".git")
		if p := strings.Split(gh, "/"); len(p) >= 2 && p[0] != "" && p[1] != "" {
			gh, remote = p[0]+"/"+p[1], ""
		} else {
			gh = ""
		}
	}
	return gh, remote, ref
}

// errNoHead is a ref no commit can be named for, now: a semver range.
var errNoHead = errors.New("its version range is bun's to resolve")

// remoteHead is the commit the git spec's branch or tag is at now; a spec
// pinned to a commit is that commit, asked of nobody. Tests stub it.
var remoteHead = func(ctx context.Context, spec string) (string, error) {
	gh, remote, ref := gitSource(spec)
	if hexSHA.MatchString(strings.ToLower(ref)) {
		return strings.ToLower(ref), nil
	}
	if strings.HasPrefix(ref, "semver:") {
		return "", errNoHead
	}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	if gh != "" {
		return githubHead(ctx, gh, ref)
	}
	if remote == "" {
		return "", fmt.Errorf("no repository in %s", spec)
	}
	return lsRemote(ctx, remote, ref)
}

// githubHead asks GitHub for the commit ref (the default branch when "")
// of owner/repo is at.
func githubHead(ctx context.Context, repo, ref string) (string, error) {
	u := githubAPI + "/repos/" + repo + "/commits?per_page=1"
	if ref != "" {
		u += "&sha=" + url.QueryEscape(ref)
	}
	b, err := fetchJSON(ctx, u, 1<<20)
	if err != nil {
		return "", err
	}
	var cs []struct {
		SHA string `json:"sha"`
	}
	if err := json.Unmarshal(b, &cs); err != nil {
		return "", fmt.Errorf("GitHub's commits of %s: %w", repo, err)
	}
	if len(cs) == 0 || !hexSHA.MatchString(cs[0].SHA) {
		return "", fmt.Errorf("GitHub named no commit of %s", repo)
	}
	return cs[0].SHA, nil
}

// lsRemote asks the repository at remote, with git, for the commit ref
// (HEAD when "") is at: an annotated tag's commit, not the tag's own.
func lsRemote(ctx context.Context, remote, ref string) (string, error) {
	args := []string{"ls-remote", remote}
	if ref == "" {
		args = append(args, "HEAD")
	} else {
		args = append(args, "refs/heads/"+ref, "refs/tags/"+ref, "refs/tags/"+ref+"^{}")
	}
	cmd := command(ctx, "git", args...)
	cmd.Env = append(env(), "GIT_TERMINAL_PROMPT=0", "GIT_SSH_COMMAND=ssh -o BatchMode=yes")
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("git ls-remote %s: %w", remote, err)
	}
	refs := map[string]string{}
	for _, l := range strings.Split(string(out), "\n") {
		if sha, name, ok := strings.Cut(strings.TrimSpace(l), "\t"); ok {
			refs[name] = sha
		}
	}
	for _, k := range []string{"HEAD", "refs/tags/" + ref + "^{}", "refs/heads/" + ref, "refs/tags/" + ref} {
		if sha := refs[k]; hexSHA.MatchString(sha) {
			return sha, nil
		}
	}
	return "", fmt.Errorf("%s has no %s", remote, map[bool]string{true: "HEAD", false: ref}[ref == ""])
}

// headTTL is how long a repository's answer stands: GitHub lets 60
// requests an hour through without a token, and the page asks on each
// visit.
const headTTL = 10 * time.Minute

type headAnswer struct {
	sha string
	err error
	at  time.Time
}

var (
	headsMu sync.Mutex
	heads   = map[string]headAnswer{}
)

// GitHead is the commit the git spec's repository is at now, asked at most
// every headTTL (fresh: asked now), "" with why when it couldn't be told.
func GitHead(ctx context.Context, spec string, fresh bool) (string, error) {
	headsMu.Lock()
	a, ok := heads[spec]
	headsMu.Unlock()
	if ok && !fresh && time.Since(a.at) < headTTL {
		return a.sha, a.err
	}
	sha, err := remoteHead(ctx, spec)
	if err != nil && ok && a.sha != "" && !fresh {
		// not reached this time: what it said last stands, till the next
		sha, err = a.sha, nil
	}
	headsMu.Lock()
	heads[spec] = headAnswer{sha, err, time.Now()}
	headsMu.Unlock()
	return sha, err
}

// GitHeadCached is what the git spec's repository said last, never asked
// now; "" when it hasn't been asked or didn't answer.
func GitHeadCached(spec string) string {
	headsMu.Lock()
	defer headsMu.Unlock()
	return heads[spec].sha
}

// GitBehind is whether the git plugin spec's repository has moved on from
// the commit installed: both known, and not the same.
func GitBehind(spec, head string) bool {
	have := GitCommit(spec)
	return have != "" && head != "" && !SameCommit(have, head)
}

// GitHeads is the commit each installed git plugin's repository is at,
// as GitHead tells it, a few asked at once; one that couldn't be told is
// left out.
func GitHeads(ctx context.Context) map[string]string {
	out := map[string]string{}
	var mu sync.Mutex
	var wg sync.WaitGroup
	sem := make(chan struct{}, npmAtOnce)
	for _, e := range Load().Plugins {
		if !IsGit(e.Spec) {
			continue
		}
		wg.Add(1)
		go func(spec string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			if sha, err := GitHead(ctx, spec, false); err == nil && sha != "" {
				mu.Lock()
				out[spec] = short(sha)
				mu.Unlock()
			}
		}(e.Spec)
	}
	wg.Wait()
	return out
}

// ShortCommit is a commit as the page shows it, its first 7 digits.
func ShortCommit(sha string) string { return short(sha) }

// short is a commit as the page shows it.
func short(sha string) string {
	if len(sha) > 7 {
		return sha[:7]
	}
	return sha
}
