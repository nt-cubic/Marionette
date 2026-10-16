package provider

// A provider the user switched off (Provider.Off) or whose account they
// removed from magpie (Provider.Hidden) is left alone in the background:
// no CLI is asked who is signed in (`claude auth status`, `cursor-agent
// about`, `devin auth status`), no allowance is read, no sign-in renewed,
// no window warmed, no account switched, no daily check-in made and no
// model list fetched. What was last known of it is still shown. Each of
// those ran every minute or so for a Claude switched off, and a request
// made just as a proxy came back up after a restart went out on the
// machine's own address (Riko on Discord).

// leftAlone says whether magpie leaves provider id alone in the
// background: switched off, or its account removed from magpie.
func leftAlone(id string) bool {
	if id == "" {
		return false
	}
	for _, p := range load().Providers {
		if p.ID == id && (p.Off || p.Hidden) {
			return true
		}
	}
	return false
}
