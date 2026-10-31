package sessions

import (
	"cmp"
	"math"
	"slices"
	"sort"
	"time"
)

// ExternalModel and ExternalSession are read-only session projections from a
// request ledger such as the gateway. They intentionally carry no file or
// resume capability.
type ExternalModel struct {
	Model string
	Tokens
	Cost   float64
	Priced bool
}
type ExternalSession struct {
	Agent, ID   string
	Title       string // what it was first asked (GatewayTitle); its id when ""
	Start, Last time.Time
	Models      []ExternalModel
	Daily       []ExternalDayUsage
	Tokens
	Cost     float64
	Unpriced int
}

// MergeExternal adds external sessions to a file-backed listing. Native
// sessions win on the exact Agent+ID identity and external entries are kept
// read-only (no file of the agent's to resume or open) with a gateway transcript
// reader; magpie's own record of one is deleted by DeleteGateway.
func MergeExternal(native []Session, external []ExternalSession) []Session {
	names := desktopExternalTitles(external)
	// keyed by the pair, not "agent|id": a string made for each of ~5000
	// sessions on each of a poll's listings, and the set and the list grown
	// an append at a time, were ~200 MB in 80s of polls (#2278)
	type agentID struct{ agent, id string }
	seen := make(map[agentID]struct{}, len(native))
	for _, s := range native {
		// GatewayKeys, as pairs
		seen[agentID{s.Agent, s.ID}] = struct{}{}
		if s.Agent == "raft" {
			seen[agentID{"pi", s.ID}] = struct{}{}
		}
		if s.App != "" {
			seen[agentID{s.App, s.ID}] = struct{}{}
		}
		for _, id := range s.Copies {
			seen[agentID{s.Agent, id}] = struct{}{}
		}
		for _, id := range s.Subs {
			seen[agentID{s.Agent, id}] = struct{}{}
		}
	}
	added := func(e ExternalSession) bool {
		_, dup := seen[agentID{e.Agent, e.ID}]
		return e.Agent != "" && e.ID != "" && !dup
	}
	n := len(native)
	for _, e := range external {
		if added(e) {
			n++
		}
	}
	out := make([]Session, len(native), n)
	copy(out, native)
	for _, e := range external {
		if !added(e) {
			continue
		}
		s := Session{ReadOnly: true, Gateway: true, Transcript: true, Agent: e.Agent, ID: e.ID, Title: cmp.Or(names[e.Agent+"|"+e.ID], e.Title, e.ID), Start: e.Start, Last: e.Last, Models: []Model{}, Unpriced: e.Unpriced}
		for _, m := range e.Models {
			s.Models = append(s.Models, Model{Model: m.Model, Tokens: m.Tokens, Cost: m.Cost, Priced: m.Priced})
			s.Tokens.add(m.Tokens)
		}
		s.Cost = e.Cost
		out = append(out, s)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Last.After(out[j].Last) })
	return out
}

// desktopExternalTitles are the names an app saved for its sessions among
// external, by Agent|ID: Claude Desktop's (DesktopTitles), PI-Desktop's
// (PIDesktopTitles, #1967). They come before the gateway's recorded first
// prompt.
func desktopExternalTitles(external []ExternalSession) map[string]string {
	var ids, pi []string
	for _, e := range external {
		if e.Agent == "claude-desktop" {
			ids = append(ids, e.ID)
		} else if isPIDesktop(e.Agent) {
			pi = append(pi, e.ID)
		}
	}
	out := map[string]string{}
	for id, name := range DesktopTitles(ids) {
		out["claude-desktop|"+id] = name
	}
	if len(pi) > 0 {
		names := PIDesktopTitles(pi)
		for _, e := range external {
			if name := names[e.ID]; name != "" && isPIDesktop(e.Agent) {
				out[e.Agent+"|"+e.ID] = name
			}
		}
	}
	return out
}

// MergeExternalStats appends external daily usage and session summaries to a
// Stats snapshot. It is deliberately data-only so callers can keep their own
// source and pricing policy.
type ExternalDayUsage struct {
	Date, Agent, Cwd, Model string
	Tokens
	Cost   float64
	Priced bool
}

// st itself is not written: its Days and Sessions may be kept for the next
// read (keptStats), so what is changed is copied first.
func MergeExternalStats(st Stats, days []ExternalDayUsage, external []ExternalSession) Stats {
	oldFirst, _ := time.ParseInLocation(time.DateOnly, st.From, time.Local)
	for _, d := range days {
		if d.Date <= st.To && (st.From == "" || d.Date < st.From) {
			st.From = d.Date
		}
	}
	first, _ := time.ParseInLocation(time.DateOnly, st.From, time.Local)
	if !oldFirst.IsZero() && !first.Equal(oldFirst) {
		// noted, not written into a copy of every session
		st.shift += int(math.Round(oldFirst.Sub(first).Hours() / 24))
	}
	st.Days = slices.Clone(st.Days)
	byDate, own := map[string]int{}, map[int]bool{}
	for i := range st.Days {
		byDate[st.Days[i].Date] = i
	}
	for _, u := range days {
		if u.Date < st.From || u.Date > st.To {
			continue
		}
		i, ok := byDate[u.Date]
		if !ok {
			st.Days = append(st.Days, Day{Date: u.Date, Usage: []Usage{}, Active: []Active{}})
			i = len(st.Days) - 1
			byDate[u.Date], own[i] = i, true
		}
		d := &st.Days[i]
		if !own[i] {
			// a day of st's: its rows are copied before one is added to
			d.Usage, own[i] = slices.Clone(d.Usage), true
		}
		found := false
		for i := range d.Usage {
			if d.Usage[i].Agent == u.Agent && d.Usage[i].Cwd == u.Cwd && d.Usage[i].Model == u.Model {
				d.Usage[i].Tokens.add(u.Tokens)
				d.Usage[i].Cost += u.Cost
				d.Usage[i].Priced = d.Usage[i].Priced && u.Priced
				found = true
				break
			}
		}
		if !found {
			d.Usage = append(d.Usage, Usage{Agent: u.Agent, Cwd: u.Cwd, Model: u.Model, Tokens: u.Tokens, Cost: u.Cost, Priced: u.Priced})
		}
	}
	names := desktopExternalTitles(external)
	// the gateway's go beside Sessions, not into a copy of them (#2278)
	st.gateway = slices.Clip(st.gateway)
	for _, e := range external {
		if e.ID == "" || e.Agent == "" {
			continue
		}
		s := Summary{Key: e.Agent + ":" + e.ID, Agent: e.Agent, ID: e.ID, Title: cmp.Or(names[e.Agent+"|"+e.ID], e.Title), Gateway: true, Last: e.Last, Tokens: e.Tokens, Cost: e.Cost, Priced: true, Models: []string{}, Days: []int{}}
		perDay := map[int]*summaryDay{}
		for _, u := range e.Daily {
			day, err := time.ParseInLocation(time.DateOnly, u.Date, time.Local)
			if err != nil {
				continue
			}
			// in Sessions' days, st.shift after the range's
			d := int(math.Round(day.Sub(first).Hours()/24)) - st.shift
			if first.IsZero() || u.Date < st.From || u.Date > st.To {
				continue
			}
			pd := perDay[d]
			if pd == nil {
				pd = &summaryDay{day: d}
				perDay[d] = pd
				s.Days = append(s.Days, d)
			}
			pd.output += u.Output
		}
		sort.Ints(s.Days)
		for _, d := range s.Days {
			s.perDay = append(s.perDay, *perDay[d])
		}
		for _, m := range e.Models {
			s.Models = append(s.Models, m.Model)
			s.Priced = s.Priced && m.Priced
		}
		st.gateway = append(st.gateway, s)
	}
	sort.Slice(st.Days, func(i, j int) bool { return st.Days[i].Date < st.Days[j].Date })
	sort.SliceStable(st.gateway, func(i, j int) bool { return st.gateway[i].Last.After(st.gateway[j].Last) })
	return st
}
