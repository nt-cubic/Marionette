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
	seen := map[string]bool{}
	for _, s := range native {
		seen[s.Agent+"|"+s.ID] = true
	}
	out := append([]Session(nil), native...)
	for _, e := range external {
		if e.Agent == "" || e.ID == "" || seen[e.Agent+"|"+e.ID] {
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

func MergeExternalStats(st Stats, days []ExternalDayUsage, external []ExternalSession) Stats {
	oldFirst, _ := time.ParseInLocation(time.DateOnly, st.From, time.Local)
	for _, d := range days {
		if d.Date <= st.To && (st.From == "" || d.Date < st.From) {
			st.From = d.Date
		}
	}
	first, _ := time.ParseInLocation(time.DateOnly, st.From, time.Local)
	if !oldFirst.IsZero() && !first.Equal(oldFirst) {
		shift := int(math.Round(oldFirst.Sub(first).Hours() / 24))
		st.Sessions = slices.Clone(st.Sessions)
		for i := range st.Sessions {
			s := &st.Sessions[i]
			s.Days = slices.Clone(s.Days)
			for j := range s.Days {
				s.Days[j] += shift
			}
			s.perDay = slices.Clone(s.perDay)
			for j := range s.perDay {
				s.perDay[j].day += shift
			}
		}
	}
	byDate := map[string]int{}
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
			byDate[u.Date] = i
		}
		d := &st.Days[i]
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
			d := int(math.Round(day.Sub(first).Hours() / 24))
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
		st.Sessions = append(st.Sessions, s)
	}
	sort.Slice(st.Days, func(i, j int) bool { return st.Days[i].Date < st.Days[j].Date })
	sort.SliceStable(st.Sessions, func(i, j int) bool { return st.Sessions[i].Last.After(st.Sessions[j].Last) })
	return st
}
