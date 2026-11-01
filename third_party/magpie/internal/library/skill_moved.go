package library

// A skill brought in from a folder an app keeps per version, which the
// app's update moved (tjhhhh on Discord, after #2635). ego lite keeps its
// skills in its framework's Versions/<v>; an update puts them in the new
// version's folder and deletes the old one. dccfefdf7 links a skill
// brought in from then on through the app's own steady link, but one
// brought in before it links straight into Versions/<old>: after the
// update the library's link points at nothing, and every agent's copy of
// it with it. followMovedSkills mends such links each time the library is
// read or synced. Only a link is replaced, never a folder: a skill that is
// a folder of the library's own, or a link that still leads somewhere
// that isn't a version the app has moved on from, is left as it is.

import (
	"log"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// followMovedSkills re-points each library skill linked into a version
// folder an app has moved on from: one gone, or one the app's
// Versions/Current no longer is. It says whether a skill was changed.
func (l *Library) followMovedSkills() bool {
	changed := false
	for _, s := range l.Skills {
		if s.Source == nil || s.Source.Kind != "folder" {
			continue
		}
		p := skillDir(s.Name)
		to, ok := linkTarget(p)
		if !ok {
			continue // the library's own folder: nothing to follow
		}
		if !filepath.IsAbs(to) {
			to = filepath.Join(filepath.Dir(p), to)
		}
		to = filepath.Clean(to)
		next := ""
		if _, err := os.Stat(p); err != nil {
			if !absent(err) {
				continue // can't be read: unknown, not gone
			}
			next = movedVersion(to)
		} else {
			next = laterCurrent(to)
		}
		if next == "" || next == to {
			continue
		}
		// os.Remove takes the link away, never what it points at
		if err := os.Remove(p); err != nil {
			log.Printf("library: following %s to %s: %v", s.Name, next, err)
			continue
		}
		if err := dirLink(next, p); err != nil {
			log.Printf("library: following %s to %s: %v", s.Name, next, err)
			if err := dirLink(to, p); err != nil {
				log.Printf("library: putting %s's link back: %v", s.Name, err)
			}
			continue
		}
		log.Printf("library: %s moved with its app, from %s to %s", s.Name, to, next)
		s.Source.Dir = next
		changed = true
	}
	return changed
}

// versionName is a folder named for a version: 0.6.0.12, v2.1, 1.4.2-beta.
var versionName = regexp.MustCompile(`^[vV]?\d+(\.\d+)*([-+_][0-9A-Za-z.]+)?$`)

// currentNames are the links an app keeps beside its version folders to
// the one in use.
var currentNames = []string{"Current", "current", "latest"}

// movedVersion is where a skill folder at p, which isn't there, is in the
// app's version now: p's first missing part is a version folder, and a
// version beside it has the same skill at the same place. The app's link
// to its current version is taken first, as it moves on with the next
// update too; then the highest version that has it. "" when there's none.
func movedVersion(p string) string {
	// up is the nearest folder above p that is there, gone the part of p
	// in it that isn't, and rest the way on from gone
	d, rest := filepath.Clean(p), ""
	for {
		up := filepath.Dir(d)
		if up == d {
			return ""
		}
		if _, err := os.Stat(up); err == nil {
			break
		} else if !absent(err) {
			return "" // can't be read: unknown
		}
		rest = filepath.Join(filepath.Base(d), rest)
		d = up
	}
	up, gone := filepath.Dir(d), filepath.Base(d)
	if !versionName.MatchString(gone) {
		return ""
	}
	for _, c := range currentNames {
		if at := filepath.Join(up, c); linked(at) && hasSkillMD(filepath.Join(at, rest)) {
			return filepath.Join(at, rest)
		}
	}
	es, err := os.ReadDir(up)
	if err != nil {
		return ""
	}
	best := ""
	for _, e := range es {
		if !versionName.MatchString(e.Name()) || !hasSkillMD(filepath.Join(up, e.Name(), rest)) {
			continue
		}
		if best == "" || versionLess(best, e.Name()) {
			best = e.Name()
		}
	}
	if best == "" {
		return ""
	}
	return filepath.Join(up, best, rest)
}

// laterCurrent is p, a folder in a bundle's Versions/<v>, named through
// Versions/Current when Current has moved on to another version that has
// the same skill at the same place: the app was updated and left the old
// version behind. "" when p isn't in such a folder.
func laterCurrent(p string) string {
	parts := strings.Split(p, string(filepath.Separator))
	for i := len(parts) - 2; i > 0; i-- {
		if parts[i] != "Versions" || !versionName.MatchString(parts[i+1]) {
			continue
		}
		vs := strings.Join(parts[:i+1], string(filepath.Separator))
		cur := filepath.Join(vs, "Current")
		if !linked(cur) || realDir(cur) == realDir(filepath.Join(vs, parts[i+1])) {
			return ""
		}
		next := filepath.Join(append([]string{cur}, parts[i+2:]...)...)
		if !hasSkillMD(next) {
			return ""
		}
		return next
	}
	return ""
}

func hasSkillMD(dir string) bool {
	fi, err := os.Stat(filepath.Join(dir, "SKILL.md"))
	return err == nil && !fi.IsDir()
}

// versionLess is whether version a comes before b, a number at a time.
func versionLess(a, b string) bool {
	num := func(s string) []int {
		s = strings.TrimLeft(s, "vV")
		if i := strings.IndexAny(s, "-+_"); i >= 0 {
			s = s[:i]
		}
		var out []int
		for _, f := range strings.Split(s, ".") {
			n, _ := strconv.Atoi(f)
			out = append(out, n)
		}
		return out
	}
	x, y := num(a), num(b)
	for i := 0; i < len(x) || i < len(y); i++ {
		var m, n int
		if i < len(x) {
			m = x[i]
		}
		if i < len(y) {
			n = y[i]
		}
		if m != n {
			return m < n
		}
	}
	return a < b
}
