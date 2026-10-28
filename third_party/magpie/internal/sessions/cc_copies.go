package sessions

import (
	"bytes"
	"cmp"
	"slices"

	"github.com/tidwall/gjson"
)

// Claude Code copies a conversation into a new session file when it forks
// one (`--resume --fork-session`, /branch) or moves it to a background
// session: every message of it again, under the same uuids, with the new
// session's id. A client that forks a session for each turn it runs leaves
// one file per fork, and each was listed as a session of its own: the same
// title, the same messages and size, ten rows of one conversation (Neo,
// #feedback). A file whose last message another file of the same agent
// holds is such a copy: everything it says is in that file too, so it is
// not a row of its own. A fork that went on with a turn of its own has a
// last message no other file holds, and stays its own row, as Claude
// Code's /resume lists it. A copy made after a compaction (/compact, or
// /rewind's summarize) holds only the compaction's summary and what
// follows it, so it starts with the summary, not with the conversation's
// first message; it is judged with the files that hold that summary.

// ccCopies is, for each session (by key) that is a copy, the key of the
// session it is a copy of, which no other holds. Only the files cache has
// read are judged: what has not been read is no one's copy.
func ccCopies(files []file) map[string]string {
	type one struct {
		f  file
		st *state
	}
	// files start alike when they start with the same message, or one
	// starts with a compaction's summary the other holds: a copy made
	// after /compact holds only what follows the summary
	var all []one
	up := []int{} // each file's group, by its index in all (union-find)
	find := func(i int) int {
		for up[i] != i {
			up[i] = up[up[i]]
			i = up[i]
		}
		return i
	}
	at := map[string]int{} // a message a file starts with or holds a summary as
	for _, f := range files {
		if !f.main || !parserFor(f.agent).claude {
			continue
		}
		st := cache[f.path]
		if st == nil || st.Root == "" || st.Leaf == "" {
			continue
		}
		i := len(all)
		all = append(all, one{f, st})
		up = append(up, i)
		in := f.agent + "\x00" + f.m().wsl + "\x00"
		for _, u := range append([]string{st.Root}, st.Compacts...) {
			if j, ok := at[in+u]; ok {
				up[find(i)] = find(j)
			} else {
				at[in+u] = i
			}
		}
	}
	groups := map[int][]one{}
	for i, o := range all {
		r := find(i)
		groups[r] = append(groups[r], o)
	}
	var out map[string]string
	used := map[string]bool{}
	for _, g := range groups {
		if len(g) < 2 {
			continue
		}
		holds := func(b one, uuid string) bool {
			used[b.f.path] = true
			return b.st.Leaf == uuid || ccUUIDsOf(b.f)[uuid]
		}
		// before reports whether b is kept over a: the one that holds
		// the other's first message too (a copy made after a compaction
		// starts with its summary, and the conversation holds what came
		// before), then the newer file, then the later key, so of two
		// files that hold each other one stays
		before := func(b, a one) bool {
			if ab, ba := holds(a, b.st.Root), holds(b, a.st.Root); ab != ba {
				return ba
			}
			if c := b.f.mod.Compare(a.f.mod); c != 0 {
				return c > 0
			}
			return cmp.Compare(b.f.key, a.f.key) > 0
		}
		hidden := make([]bool, len(g))
		for i, a := range g {
			for j, b := range g {
				if i == j || a.f.key == b.f.key || !holds(b, a.st.Leaf) {
					continue
				}
				if !holds(a, b.st.Leaf) || before(b, a) {
					hidden[i] = true
					break
				}
			}
		}
		for i, a := range g {
			if !hidden[i] {
				continue
			}
			// the file it is kept in: one that is not a copy itself and
			// holds its last message, as the longest of them does
			for j, b := range g {
				if !hidden[j] && i != j && holds(b, a.st.Leaf) {
					if out == nil {
						out = map[string]string{}
					}
					out[a.f.key] = b.f.key
					break
				}
			}
		}
	}
	// a file judged here that no longer starts like another lets its
	// uuids go; another caller's files are left as they are
	for _, g := range groups {
		for _, o := range g {
			if !used[o.f.path] {
				delete(ccUUIDMemo, o.f.path)
			}
		}
	}
	return out
}

// ccUUIDMemo is each file's message uuids as ccUUIDsOf read them, kept
// while the file is one of a pair that start alike. Under mu.
var ccUUIDMemo = map[string]*ccUUIDs{}

type ccUUIDs struct {
	size, mod, off int64
	set            map[string]bool
}

// ccUUIDsOf is the uuids of the messages in f, read on from where the last
// read stopped while the file only grew.
func ccUUIDsOf(f file) map[string]bool {
	m := ccUUIDMemo[f.path]
	if m == nil || f.size < m.size || f.size == m.size && f.mod.UnixNano() != m.mod {
		m = &ccUUIDs{set: map[string]bool{}}
		ccUUIDMemo[f.path] = m
	}
	if m.size == f.size && m.mod == f.mod.UnixNano() {
		return m.set
	}
	off, err := scanThin(f.path, m.off, nil, thinClaude, func(b []byte, _, _ int64) bool {
		if !bytes.Contains(b, ccUser) && !bytes.Contains(b, ccAssistant) {
			return true
		}
		r := gjson.GetManyBytes(b, "type", "uuid")
		if t := r[0].String(); (t == "user" || t == "assistant") && r[1].Type == gjson.String {
			m.set[r[1].String()] = true
		}
		return true
	})
	if err == nil {
		m.off = off
	}
	m.size, m.mod = f.size, f.mod.UnixNano()
	return m.set
}

// copiesByKeeper is the ids of the copies kept in each session, by its key.
func copiesByKeeper(copies map[string]string, groups map[string][]file) map[string][]string {
	if len(copies) == 0 {
		return nil
	}
	out := map[string][]string{}
	for k, keeper := range copies {
		if g := groups[k]; len(g) > 0 {
			out[keeper] = append(out[keeper], sessionID(g))
		}
	}
	for _, ids := range out {
		slices.Sort(ids)
	}
	return out
}
