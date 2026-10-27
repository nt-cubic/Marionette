package provider

import (
	"encoding/json"
	"errors"
	"time"
)

// WorkBuddy's and WorkBuddy AI's accounts on
// @magpie-community/opencode-workbuddy-auth (one package, a provider for
// each build). The desktop app's own sign-in goes as a marker ({source:
// "desktop"}, the package from 0.1.1 on): the plugin reads the app's file
// on each request, as the built-in does, so no refresh token is held by
// both. An account magpie signed in goes whole.
func init() {
	for _, w := range []*wbSite{wbCN, wbAI} {
		movers[w.id] = &mover{
			pkg:    "@magpie-community/opencode-workbuddy-auth",
			min:    "0.1.37", // 0.1.37: sign in with a WorkBuddy API key from the profile/keys page, sent as WorkBuddy's own client sends it (plugins #407); 0.1.35: used credits say when the plan's cycle starts again, as WorkBuddy's client does, so the card no longer reads "Not used yet" beside them (windy on Discord); 0.1.34: a free account's plan reads "Free plan" (plugins #337); 0.1.33: an untyped object oneOf goes to Kimi as type "object" (plugins #378); 0.1.32: plugins #346; 0.1.31: a 401 is renewed once, whatever the token's time, and sent once more; when the retry is refused too, the 401 says to sign in again (Providers → WorkBuddy → this account), and the account is marked expired only on WorkBuddy's word that the sign-in is gone (refresh token refused, or invalid_token with nothing to renew with) (wenchen855443 on X); 0.1.30: a refused line the model quotes back into an assistant message is reworded there too, so a long DeepSeek Harness chat isn't refused for good (plugins #291); 0.1.29: a system prompt WorkBuddy refuses as an unapproved channel (11128) is sent once more as the first user message's instructions, with WorkBuddy's default system prompt, since WorkBuddy checks only system and assistant messages; the refusal still rests no account (Wing on Discord); 0.1.28: a refusal of words not reworded yet ends in a hint that names no one client (plugins #279); 0.1.27: DeepSeek Harness's opening identity line is reworded to a neutral one, which WorkBuddy otherwise refuses as an unapproved channel (11128) (plugins #279, #280); 0.1.26: a model that says only supportsReasoning and a default effort lists low, high, max and that default, not "doesn't reason" (plugins #255); 0.1.25: WorkBuddy AI's fast, balanced and primary models keep the credit rate its config gives as "x0.34 credits", not unknown (plugins #216); 0.1.24: a refused sign-in on a chat over 2 MiB says the 401 and to sign in again: WorkBuddy's front door resets such a request instead of answering, so it read as 502 ECONNRESET (wyp on Discord); 0.1.23: credits used up leave the free models counted out of the credits window, so an account is not ranked spent for them (plugins #138); 0.1.22: an MCP server's tool schemas reach Kimi and DeepSeek: an enum with no type, or of several types, is typed, and true or false where a schema goes is {} or {"not": {}}, so they aren't 400 11133 "Invalid request parameters" (plugins #122); 0.1.21: a model free only at night (Hy4 preview) is never said free, and a whole-day discount shows the price before it, from WorkBuddy's modelPromotions (plugins #121); 0.1.20: tool results followed by the user's message reach WorkBuddy AI's gemini-3.8-flash, not 400 "the conversation must end with a user message" (plugins #113); 0.1.19: an enterprise account's card shows its credits, from the enterprise meter WorkBuddy's usage page reads (#1904); 0.1.18: WorkBuddy 5.6+ keeps its desktop sign-in encrypted, and the plugin says so and points to the browser sign-in instead of a 502 (plugins #18); 0.1.17: a restricted account's 403 11140 ends in provider.WBRestrictedHint, so it rests 2h as restricted (#1849); 0.1.16: WorkBuddy AI's GLM models are text-only, so magpie's image-describing model reads their images (Yirasumi on Discord); 0.1.15: a system prompt sent as a developer message passes the channel check, an object tool_choice is sent as WorkBuddy takes it, and max_completion_tokens goes as max_tokens so the cap holds (plugins #96); 0.1.14: the Credits window says when the soonest credits left run out, for Smart routing to spend them first (cdredfox on X); an enterprise account names its enterprise, so it lists and uses the enterprise's models, not the personal plan's (plugins #75); a chat's tools get shapes WorkBuddy AI's models take, so ZCode's and DeepSeek Harness's chats on deepseek-v4.1-flash aren't 400 "Invalid request parameters" (wyh on Discord); one user send is one request in the usage list: every step carries one X-Conversation-Request-ID, and both builds name the client (使用端) (plugins #48); credits used up (code 14018) say so, with the nested error message (plugins #25); Codex's and Claude Code's chats pass WorkBuddy's channel check (#182); a failure's status and its sign-in mark as the built-in's; each model's credit rate; the sign-in renewed ahead of time through auth.refresh; the credits' count beside their share (#659)
			agents: []string{w.id},
			out: func() ([]Moving, error) {
				var out []Moving
				for _, a := range wbLogins(w) {
					m := Moving{User: a.User, ID: a.creds.UID, First: a.Active, On: a.On, Lapsed: a.Lapsed != "", Own: a.own, Plan: a.Plan}
					if a.own {
						m.Auth = map[string]any{"type": "oauth", "source": "desktop", "access": "", "refresh": "", "expires": 0,
							"accountId": a.User, "uid": a.creds.UID}
					} else {
						c := a.creds
						m.Auth = map[string]any{"type": "oauth", "access": c.Access, "refresh": c.Refresh, "expires": c.ExpiresAt,
							"accountId": a.User, "uid": c.UID, "domain": c.Domain}
						if c.RefreshExpiresAt > 0 {
							m.Auth["refreshExpiresAt"] = c.RefreshExpiresAt
						}
						if c.TokenType != "" {
							m.Auth["tokenType"] = c.TokenType
						}
					}
					out = append(out, m)
				}
				return out, nil
			},
			back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
				if str(auth["source"]) != "" {
					// the desktop app's: nothing of it was the plugin's
					return ls, firstNonEmpty(user, str(auth["accountId"])), nil
				}
				c := wbCreds{UID: str(auth["uid"]), Access: str(auth["access"]), Refresh: str(auth["refresh"]),
					ExpiresAt: msOf(auth["expires"]), RefreshExpiresAt: msOf(auth["refreshExpiresAt"]),
					Domain: str(auth["domain"]), TokenType: str(auth["tokenType"])}
				if c.UID == "" || c.Access == "" {
					return ls, "", errors.New(w.name + ": an unreadable plugin sign-in")
				}
				if user == "" {
					user = str(auth["accountId"])
				}
				i := backInto(&ls, w.id, user)
				b, err := json.Marshal(c)
				if err != nil {
					return ls, "", err
				}
				ls[i].Auth, ls[i].Lapsed, ls[i].Seen = b, "", time.Now().UTC().Truncate(time.Second)
				wbTokens.Lock()
				delete(wbTokens.m, w.id+"|"+c.UID)
				wbTokens.Unlock()
				return ls, ls[i].User, nil
			},
		}
	}
}

// msOf is a time in unix ms as JSON gave it.
func msOf(v any) int64 {
	switch n := v.(type) {
	case float64:
		return int64(n)
	case int64:
		return n
	case int:
		return int64(n)
	}
	return 0
}
