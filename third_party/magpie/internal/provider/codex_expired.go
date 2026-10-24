package provider

// A ChatGPT sign-in the backend calls expired while its access token has
// time left by its own exp (willz on Discord, #2973: two saved Team
// accounts, /wham/usage answering 401 "Provided authentication token is
// expired. Please try refreshing it." until each was signed in again).
// magpie refreshes a token only as its exp nears, so it kept sending the
// one turned away. Such an answer is mended by one refresh: forced here,
// and the read made once more with the new token, never more.

import (
	"context"
	"errors"
	"log"
	"net/http"
	"regexp"
	"strings"
	"time"
)

// expiredWords: a 401 that says the access token has expired and asks
// for a refresh. Not refresh_token_expired ("_" is a word character, so
// \b doesn't split it there), which says the sign-in itself is gone
// (revokedWords).
var expiredWords = regexp.MustCompile(`(?i)\btoken_expired\b|authentication token (is|has) expired|please try refreshing it`)

// SignInExpired says a reply is the vendor's word that the access token
// it was sent with has expired, for a refresh to mend.
func SignInExpired(status int, body []byte) bool {
	return status == http.StatusUnauthorized && expiredWords.Match(body)
}

// codexRenew refreshes a ChatGPT sign-in in place of stale, the access
// token the backend called expired (codexTokenRenewing,
// renewSavedLoginFrom).
type codexRenew func(ctx context.Context, stale string) (tok, accountID string, err error)

// codexExpired says err is the backend calling the token expired.
func codexExpired(err error) bool {
	var se *accountStatusError
	return errors.As(err, &se) && SignInExpired(se.status, se.body)
}

// errCodexStillExpired is a usage read turned away as expired again with
// the token refreshed just for it: nothing magpie holds will do.
var errCodexStillExpired = errors.New("ChatGPT turned the refreshed sign-in away too; sign in to this account again")

// codexWindowsRenewing is codexWindows, read once more with the sign-in
// renewed when the backend calls token expired. A refresh refused says
// to sign in again (codexRefresh, savedRefreshFailed).
func codexWindowsRenewing(ctx context.Context, tok, accountID string, renew codexRenew) (plan string, out []QuotaWindow, resets *ResetCredits, credits string, held bool, err error) {
	plan, out, resets, credits, held, err = codexWindows(ctx, tok, accountID)
	if renew == nil || !codexExpired(err) {
		return
	}
	fresh, id, rerr := renew(ctx, tok)
	if rerr != nil {
		return plan, out, resets, credits, held, rerr
	}
	if id != "" {
		accountID = id
	}
	plan, out, resets, credits, held, err = codexWindows(ctx, fresh, accountID)
	if codexExpired(err) {
		err = errCodexStillExpired
	}
	return
}

// NoteExpiredSignIn renews the ChatGPT sign-in of a, whose request req the
// backend answered 401 token expired while the token had time left by
// its exp, so the next request goes with a new one. The request itself
// isn't sent again here; the routing loop passes over it as an auth
// failure. A refresh refused marks a saved account lapsed
// (savedRefreshFailed).
func NoteExpiredSignIn(ctx context.Context, a *Account, req *http.Request) {
	if a == nil || a.renew == nil || a.plugin != nil || req == nil {
		return
	}
	stale, ok := strings.CutPrefix(req.Header.Get("Authorization"), "Bearer ")
	if !ok || stale == "" {
		return
	}
	rctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 20*time.Second)
	defer cancel()
	if _, _, err := a.renew(rctx, stale); err != nil {
		log.Printf("codex: %s's token was called expired and couldn't be renewed: %v", a.User, err)
	}
}
