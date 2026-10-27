package gateway

import (
	"net/http"
	"regexp"
	"strings"
)

// zcodeEndedWords and zcodeSpentWords are the ZCode plugin's own answers
// for a Start Plan trial entry (<model>-Trial) it can't serve, given before
// anything is sent: a 400 "ZCode has no active limited quota for
// GLM-5.3-Trial" when no live plan holds the model any more (the trial
// ended), and a 429
// "ZCode's limited quota for glm-5.3 is exhausted or temporarily
// unavailable" when its bucket is spent. The plugin's words, after any
// "<provider>: " put in front of them.
var (
	zcodeEndedWords = regexp.MustCompile(`(?:^|: )ZCode has no active limited quota for (\S+)$`)
	zcodeSpentWords = regexp.MustCompile(`(?:^|: )ZCode's limited quota for (\S+) is exhausted or temporarily unavailable$`)
)

// zcodeTrialRefused says c, a ZCode plugin account asked for one of its
// Start Plan trial entries, was refused for that entry's own quota
// (zcodeEndedWords, zcodeSpentWords), naming the model asked for. The
// trial is that model's: the account's GLM Coding Plan still serves its
// own models, so only the trial entry rests (#3661, orion-nebula-dev: two
// expired trials rested "zcode@zcode", and GLM-5.3 and GLM-5.3-Flash with
// it). Any other
// refusal of a ZCode account, an account-wide 429 or 1113 among them,
// rests the account as before.
func zcodeTrialRefused(c candidate, status int, body []byte) bool {
	if c.p.PluginProvider() != "zcode" || !strings.HasSuffix(strings.ToLower(c.model), "-trial") {
		return false
	}
	said := strings.TrimSpace(vendorMessage(body))
	switch status {
	case http.StatusBadRequest:
		m := zcodeEndedWords.FindStringSubmatch(said)
		return m != nil && strings.EqualFold(m[1], c.model)
	case http.StatusTooManyRequests:
		// the plugin names the bucket's own model, the entry less -Trial
		m := zcodeSpentWords.FindStringSubmatch(said)
		return m != nil && strings.EqualFold(m[1]+"-Trial", c.model)
	}
	return false
}
