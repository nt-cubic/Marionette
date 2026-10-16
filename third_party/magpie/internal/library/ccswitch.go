package library

import (
	"os"
	"path/filepath"

	"github.com/yetone/magpie/internal/provider"
)

// ccSwitchOrigin is where on GitHub CC Switch got a skill the library has
// from CC Switch's folder — linked to there, or a copy CC Switch gave an
// agent that was taken in — so that it can be updated from GitHub like one
// installed here. CC Switch's own folder is only ever read.
func ccSwitchOrigin(s *Skill) (Source, bool) {
	dir := filepath.Join(provider.CCSwitchSkillsDir(), s.Name)
	switch {
	case s.Source == nil:
		if _, err := os.Stat(filepath.Join(dir, "SKILL.md")); err != nil {
			return Source{}, false
		}
	case s.Source.Kind == "folder":
		d := realDir(s.Source.Dir)
		if realDir(filepath.Dir(d)) != realDir(provider.CCSwitchSkillsDir()) {
			return Source{}, false
		}
		dir = d
	default:
		return Source{}, false
	}
	repo, ref, ok := provider.CCSwitchSkillOrigin(filepath.Base(dir))
	if !ok || !repoRe.MatchString(repo) {
		return Source{}, false
	}
	return Source{Kind: "github", Repo: repo, Ref: ref, Path: filepath.Base(dir)}, true
}

// origin finds the skill in the repository fetched for it: the folder
// named as CC Switch named it, or the skill of that name.
func origin(root string, src Source, name string) (string, bool) {
	for _, c := range candidates(root, "") {
		if lastPart(c.Path) == src.Path || c.Name == name {
			return c.Path, true
		}
	}
	return "", false
}
