package provider

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"
)

// farZone is a zone whose clock is hours off this computer's at the week
// the tests use: Asia/Shanghai (gaubeebangeel on X: magpie on a server in
// UTC, its user in Beijing), or America/New_York on a computer that keeps
// Beijing's time, so a window read on the computer's clock is visibly wrong.
func farZone(t *testing.T) (string, *time.Location) {
	t.Helper()
	for _, name := range []string{"Asia/Shanghai", "America/New_York"} {
		loc, err := time.LoadLocation(name)
		if err != nil {
			t.Fatal(err)
		}
		at := time.Date(2026, 9, 28, 12, 0, 0, 0, loc)
		_, a := at.Zone()
		_, b := at.In(time.Local).Zone()
		if d := a - b; d >= 4*3600 || d <= -4*3600 {
			return name, loc
		}
	}
	t.Fatal("no zone far from this computer's")
	return "", nil
}

// A window set in a zone holds at its hours there, whatever the clock of
// the computer magpie runs on: the instant is given in UTC, as a server in
// UTC reads it.
func TestRuleTimeWindowInItsZone(t *testing.T) {
	name, loc := farZone(t)
	// wall is a time on the zone's own clock, the week of Monday 2026-09-28
	// (day 0 is Sunday), as a server in UTC has it
	wall := func(day, hour, min int) time.Time { return time.Date(2026, 9, 27+day, hour, min, 0, 0, loc).UTC() }
	day := TimeWindow{From: "09:00", To: "18:00", Days: []string{"mon", "tue", "wed", "thu", "fri"}, Zone: name}
	night := TimeWindow{From: "22:00", To: "08:00", Days: []string{"fri"}, Zone: name}
	monday := TimeWindow{From: "00:00", To: "00:00", Days: []string{"mon"}, Zone: name}
	for _, c := range []struct {
		w    TimeWindow
		at   time.Time
		want bool
	}{
		{day, wall(2, 10, 0), true},     // Tuesday 10:00 in the zone
		{day, wall(2, 9, 0), true},      // as it opens
		{day, wall(2, 8, 59), false},    // before: not 8 hours off
		{day, wall(2, 18, 0), false},    // as it ends
		{day, wall(2, 17, 59), true},    // before it ends
		{day, wall(6, 10, 0), false},    // Saturday there
		{night, wall(5, 23, 0), true},   // Friday night there
		{night, wall(6, 3, 0), true},    // Saturday's early hours: Friday's window
		{night, wall(6, 8, 0), false},   // as it ends
		{night, wall(5, 3, 0), false},   // Friday's early hours: Thursday's window
		{monday, wall(1, 0, 30), true},  // Monday just begun there, Sunday in UTC
		{monday, wall(1, 23, 30), true}, // Monday's end there, Tuesday in UTC for New York
		{monday, wall(0, 23, 30), false},
		{monday, wall(2, 0, 30), false},
	} {
		if got := c.w.Holds(c.at); got != c.want {
			t.Errorf("%s at %s there (%s UTC): %v, want %v", c.w.Text(), c.at.In(loc).Format("Mon 15:04"), c.at.Format("Mon 15:04"), got, c.want)
		}
	}
	// a pause rule set there holds at the same instants
	p := Rule{Use: "a/x", Pause: true, Time: &day}
	if !p.Pauses("claude", "", wall(2, 10, 0)) || p.Pauses("claude", "", wall(2, 19, 0)) {
		t.Errorf("pause in %s: not at its hours", name)
	}
}

// A window with no zone — every one saved before zones were kept — is on
// this computer's clock, as it was.
func TestRuleTimeWindowWithoutZoneIsLocal(t *testing.T) {
	w := TimeWindow{From: "09:00", To: "18:00", Days: []string{"tue"}}
	if !w.Holds(time.Date(2026, 9, 29, 10, 0, 0, 0, time.Local).UTC()) || w.Holds(time.Date(2026, 9, 29, 8, 0, 0, 0, time.Local).UTC()) {
		t.Error("a window without a zone is not on the computer's clock")
	}
	b, _ := json.Marshal(Rule{Use: "a/x", Time: &w})
	if strings.Contains(string(b), "zone") {
		t.Errorf("no zone written for none: %s", b)
	}
}

// A zone magpie doesn't know (a file edited by hand) is an unknown clock:
// the window never holds — neither always nor at hours a day's half off —
// and saving one is refused.
func TestRuleTimeWindowUnknownZone(t *testing.T) {
	w := TimeWindow{From: "00:00", To: "00:00", Days: []string{"mon", "tue", "wed", "thu", "fri", "sat"}, Zone: "Mars/Olympus_Mons"}
	for d := range 7 {
		for h := 0; h < 24; h += 3 {
			if at := time.Date(2026, 9, 27+d, h, 0, 0, 0, time.UTC); w.Holds(at) {
				t.Fatalf("an unknown zone holds at %s", at)
			}
		}
	}
	if _, err := cleanRules([]Rule{{Use: "a/x", Time: &TimeWindow{From: "09:00", To: "18:00", Zone: "Mars/Olympus_Mons"}}}, []string{"a/x"}); err == nil || !strings.Contains(err.Error(), "Mars/Olympus_Mons") {
		t.Errorf("an unknown zone saved: %v", err)
	}
	rs, err := cleanRules([]Rule{{Use: "a/x", Time: &TimeWindow{From: "9:00", To: "18:00", Zone: " Asia/Shanghai "}}}, []string{"a/x"})
	if err != nil || rs[0].Time.Zone != "Asia/Shanghai" {
		t.Fatalf("%v %+v", err, rs)
	}
	if got := strings.Join(rs[0].Conditions(), " · "); got != "time 09:00–18:00 (Asia/Shanghai)" {
		t.Errorf("conditions %q", got)
	}
}

// Typed: tz=, or time=…@zone; read back as tz=. A zone magpie doesn't know
// is refused with what one looks like, and tz= alone names no hours.
func TestParseRuleTimeZone(t *testing.T) {
	g := Group{ID: "g", Members: []string{"a/x", "b/y"}}
	for _, c := range []struct{ line, want, cond string }{
		{"pause=b/y time=09:00-18:00 days=mon-fri tz=Asia/Shanghai", "pause=b/y time=09:00-18:00 days=mon-fri tz=Asia/Shanghai", "time 09:00–18:00 Mon–Fri (Asia/Shanghai)"},
		{"use=b/y time=09:00-18:00@Asia/Shanghai", "use=b/y time=09:00-18:00 tz=Asia/Shanghai", "time 09:00–18:00 (Asia/Shanghai)"},
		{"use=b/y tz=Europe/Berlin days=sat,sun", "use=b/y days=sat,sun tz=Europe/Berlin", "time all day Sat,Sun (Europe/Berlin)"},
		{"use=b/y days=mon time=22:00-08:00@UTC", "use=b/y time=22:00-08:00 days=mon tz=UTC", "time 22:00–08:00 Mon (UTC)"},
	} {
		r, _, _, err := ParseRule(g, RuleWords(c.line))
		if err != nil {
			t.Fatalf("%s: %v", c.line, err)
		}
		rs, err := cleanRules([]Rule{r}, g.Members)
		if err != nil {
			t.Fatalf("%s: %v", c.line, err)
		}
		if got := rs[0].Line(); got != c.want {
			t.Errorf("%s: line %q, want %q", c.line, got, c.want)
		}
		if got := strings.Join(rs[0].Conditions(), " · "); got != c.cond {
			t.Errorf("%s: %q, want %q", c.line, got, c.cond)
		}
		again, _, _, err := ParseRule(g, RuleWords(rs[0].Line()))
		if err != nil || again.Time == nil || again.Time.Zone != rs[0].Time.Zone {
			t.Errorf("%s: read back %v %+v", c.line, err, again.Time)
		}
	}
	for _, c := range []struct{ line, says string }{
		{"use=b/y time=09:00-18:00 tz=Beijing", "Beijing"},
		{"use=b/y time=09:00-18:00@Asia/Shangai", "Asia/Shangai"},
		{"use=b/y time=09:00-18:00 tz=", "IANA"},
		{"use=b/y tokens=10 tz=Asia/Shanghai", "time= or days="},
	} {
		if _, _, _, err := ParseRule(g, RuleWords(c.line)); err == nil || !strings.Contains(err.Error(), c.says) {
			t.Errorf("%s: %v, want it to say %q", c.line, err, c.says)
		}
	}
}

// A window typed with no zone takes this computer's ($TZ, or what
// /etc/localtime links to), and none when that has no IANA name.
func TestRuleInZoneHere(t *testing.T) {
	g := Group{ID: "g", Members: []string{"a/x", "b/y"}}
	typed := func(line string) string {
		r, _, _, err := ParseRule(g, RuleWords(line))
		if err != nil {
			t.Fatal(err)
		}
		r.InZoneHere()
		if r.Time == nil {
			return "-"
		}
		return r.Time.Zone
	}
	t.Setenv("TZ", "Asia/Tokyo")
	if z := typed("use=b/y time=09:00-18:00"); z != "Asia/Tokyo" {
		t.Errorf("TZ=Asia/Tokyo: %q", z)
	}
	if z := typed("use=b/y time=09:00-18:00 tz=Europe/Berlin"); z != "Europe/Berlin" {
		t.Errorf("a zone typed is kept: %q", z)
	}
	if z := typed("use=b/y tokens=5"); z != "-" {
		t.Errorf("no hours, no window: %q", z)
	}
	t.Setenv("TZ", ":Europe/Paris")
	if z := typed("use=b/y days=sat"); z != "Europe/Paris" {
		t.Errorf("TZ=:Europe/Paris: %q", z)
	}
	for _, tz := range []string{"UTC+8", "Local", ""} {
		t.Setenv("TZ", tz)
		if z := typed("use=b/y time=09:00-18:00"); z != "" {
			t.Errorf("TZ=%q names no zone: %q", tz, z)
		}
	}
	os.Unsetenv("TZ") // t.Setenv puts it back
	if z := LocalZone(); z != "" && !ValidZone(z) {
		t.Errorf("the system's zone %q is not one magpie knows", z)
	}
}
