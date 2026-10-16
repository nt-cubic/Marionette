// Package ghtoken has magpie's requests to GitHub's REST API carry the
// user's GitHub token, and says plainly when GitHub turns them away.
//
// GitHub's API answers an address 60 requests an hour without a token,
// which a check of a few repositories, or a network shared with others,
// uses up; with a token it answers 5,000 an hour. The token is the one set
// in Settings (GitHubToken), else GITHUB_TOKEN or GH_TOKEN from the
// environment. It goes to GitHub's API's host alone, never to codeload, raw
// files or a mirror (source.Do sends no request with Authorization to one),
// and is never written to a log or an error. The library's skills and the
// Plugins page's git plugins both ask GitHub through here (#1942).
package ghtoken

import (
	"fmt"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// Env names the environment variables a token is taken from when Settings
// has none, in the order they are looked at.
var Env = []string{"GITHUB_TOKEN", "GH_TOKEN"}

// Token is the token requests to GitHub's API carry, and where it came
// from: "settings", the environment variable's name, or "" for none.
func Token() (token, from string) {
	if t := strings.TrimSpace(settings.Load().GitHubToken); t != "" {
		return t, "settings"
	}
	for _, k := range Env {
		if t := strings.TrimSpace(os.Getenv(k)); t != "" {
			return t, k
		}
	}
	return "", ""
}

// Authorize has req carry the token, if there is one and req goes to the
// host of api (GitHub's API's base URL, which tests move); it says whether
// it did.
func Authorize(req *http.Request, api string) bool {
	u, err := url.Parse(api)
	if err != nil || u.Host == "" || req.URL == nil || req.URL.Host == "" || !strings.EqualFold(req.URL.Host, u.Host) {
		return false
	}
	t, _ := Token()
	if t == "" {
		return false
	}
	req.Header.Set("Authorization", "Bearer "+t)
	return true
}

// Refused is the error for a 401 from GitHub to a request that carried the
// token: which token it was, never the token itself.
func Refused() error {
	_, from := Token()
	where := "the GitHub token in Settings → Network and sharing"
	if from != "settings" {
		where = from + " (no GitHub token is set in Settings → Network and sharing)"
	}
	return fmt.Errorf("GitHub refused %s: it may have expired or been revoked", where)
}

// Limited is GitHub turning requests away for a while: until when (zero
// when GitHub didn't say), and whether the requests carried a token.
type Limited struct {
	Until time.Time
	Token bool
}

func (e Limited) Error() string {
	when := "for a while"
	if !e.Until.IsZero() {
		when = "until " + e.Until.Local().Format("15:04")
	}
	if e.Token {
		return "GitHub's rate limit for your GitHub token is used up " + when + "; try again then"
	}
	return "GitHub's rate limit for requests without a token (60 an hour) is used up " + when +
		". Add a GitHub token in Settings → Network and sharing to raise it to 5,000 an hour"
}

// LimitedBy is the Limited a refusal of GitHub's says, or false when it
// isn't one: 429, or 403 with the limit spent or a secondary limit's
// Retry-After or message. A 403 for anything else (a token without access)
// isn't one.
func LimitedBy(resp *http.Response, body []byte, token bool) (Limited, bool) {
	msg := strings.ToLower(string(body))
	if resp.StatusCode != 429 && (resp.StatusCode != 403 || resp.Header.Get("X-RateLimit-Remaining") != "0" &&
		resp.Header.Get("Retry-After") == "" && !strings.Contains(msg, "rate limit")) {
		return Limited{}, false
	}
	e := Limited{Token: token}
	if n, err := strconv.ParseInt(resp.Header.Get("X-RateLimit-Reset"), 10, 64); err == nil && n > 0 {
		e.Until = time.Unix(n, 0)
	} else if s, err := strconv.Atoi(resp.Header.Get("Retry-After")); err == nil && s > 0 {
		e.Until = time.Now().Add(time.Duration(s) * time.Second)
	}
	return e, true
}
