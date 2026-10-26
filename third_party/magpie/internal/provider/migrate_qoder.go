package provider

import (
	"encoding/json"
	"errors"
	"time"

	"github.com/yetone/magpie/internal/qoder"
)

// Qoder's and Qoder CN's accounts on @magpie-community/opencode-qoder-auth,
// which serves both sites as providers "qoder" and "qoder-cn", as its device
// sign-in keeps one: the job token pair as access and refresh (the refresh
// token is spent on use, so it moves rather than being copied), the device
// token pair and machine id beside them. A Qoder CN account that chats on
// its device token (DeviceChat) says so, its device pair as access and
// refresh.
func init() {
	for _, id := range []string{"qoder", QoderCNID} {
		movers[id] = qoderMover(id)
	}
}

func qoderMover(id string) *mover {
	return &mover{
		pkg:    "@magpie-community/opencode-qoder-auth",
		min:    "0.2.37", // 0.2.37: a QwenWork personal account's usage shows the credits left as its balance, with the daily, monthly and long-term pools from its wallets, as QwenWork's client reads them, instead of "the account reports no credits" (plugins #385); 0.2.36: tool calls written in the text are read as calls (plugins #323); 0.2.35: an untyped object oneOf goes as type "object" (plugins #378); 0.2.34: QwenWork (plugins #319); 0.2.33: a task's business.name is the prompt's first words, as Qoder.app 0.4.3 names it, not "magpie session" (plugins #354); 0.2.32: a check-in asked with this machine's own device identity and offered nothing says so, and that Qoder desktop can claim it, instead of a bare inactive (plugins #304); 0.2.31: every call of one prompt's tool loop is one Qoder task (one request_set_id, business.id and begin_at; stage start, then processing), as Qoder.app's client sends it, and a harness's session id names the session (plugins #330); 0.2.30: Qoder CN's daily check-in sends Qoder CN's own device identity (its runtime-info, environment 0) when Qoder CN desktop or CLI is installed, and says to install it when nothing is offered without one (#2911, plugins #304); 0.2.28: a request Qoder queues (10605) waits its turn in Qoder's queue, asking its queue status as Qoder's client does, and is sent again, for up to 4 minutes, before the 503 (wallance on Discord); 0.2.27: a check-in on a computer without Qoder's runtime-info helper settles for the day with what to install, instead of retrying all day (plugins #304); 0.2.26: a Codex request marks its prompt for Qoder's cache as Qoder's client does, so the Ultimate model's cache hits are no longer 0% (plugins #290), and the intl check-in finds a machine-wide Windows install's runtime info under Program Files; 0.2.25: the international sign-in sends no redirect_uri, so a Mac with Qoder.app installed finishes in the browser tab instead of being handed to the desktop app (plugins #276); 0.2.24: Qoder's 400 "Error in upstream response" (the model's own vendor failing) is a 502, so magpie asks again or falls back, not the client's fault (SlouchyQuill507 on Discord); 0.2.23: Cosy-Version claims the desktop app's worker runtime, 1.1.69, beside its desktop client type and machine identity (plugins #281); 0.2.22: a tool result whose call lost its id answers that call, not an orphan Qoder refuses (plugins #277, #278); 0.2.21: Qoder CN signs in with a personal access token, and a PAT account gets the daily check-in; 0.2.20: unnamed tool-call fragments stay out of the chat history; 0.2.19: a model list that can't be read is a 502, so a group falls back to its next member (plugins #249); 0.2.18: a 10605 trimmed or wrapped is the model's queue busy, not a lapsed sign-in (plugins #185); 0.2.17: the daily check-in through auth.checkin (plugins #179); 0.2.16: a tool call written into the text that can't be read, or that the stream ends inside, ends the turn as an error (a 502, or the stream's error frame with no finish), never as the answer (plugins #132); 0.2.14: Qoder's 403 code 10605 (the model's queue busy) is a 503 with Retry-After, not a lapsed sign-in; quota and credit codes (110, 112–119, 122) are 429, refusals of the request (406, 416, 430, 80411, 80412, 100400, 100401, 100403) are 400, and only 105 or no code is "sign in again" (plugins #98); 0.2.13: models are listed from Qoder's app scene first, so its new default Fast 0.8× shows (plugins #97); 0.2.12: a chat Qoder refuses keeps Qoder's own reason in the error; 0.2.11: a Qoder CN enterprise VPC account keeps its model list and renewal mark apart from a public account with the same uid (plugins#40); international check-in finds the device identity runtime Qoder CLI caches on macOS and Linux and the Linux desktop installs in /opt/Qoder (ARNO on Discord); sends Qoder's native device identity, without which the daily 100 credits aren't listed (#1268); Qoder CN enterprise VPC (0.2.8); the daily credits claimed through the account's fetch (ARNO on Discord); Qoder CN as "qoder-cn", its device-token chat as the built-in's; each model's credit rate; the sign-in renewed ahead of time through auth.refresh; token usage read; each model's context the largest window Qoder offers it (#722); the window a request is sent holds the reply's max_tokens (#700)
		agents: []string{id},
		out: func() ([]Moving, error) {
			// nothing else writes the pairs while they are read
			ls := qoderLoginsOf(id)
			qoderMu.Lock()
			defer qoderMu.Unlock()
			var out []Moving
			for _, l := range ls {
				c, ok := qoderSaved(l.saved)
				if !ok {
					continue
				}
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.Lapsed != "", Plan: l.Plan, Auth: map[string]any{
					"type":          "oauth",
					"access":        c.Token,
					"refresh":       c.RefreshToken,
					"expires":       c.ExpiresAt,
					"accountId":     l.User,
					"uid":           c.UID,
					"email":         c.Email,
					"name":          c.Name,
					"machineId":     c.MachineID,
					"deviceToken":   c.DeviceToken,
					"deviceRefresh": c.DeviceRefresh,
				}})
				if c.DeviceChat {
					out[len(out)-1].Auth["deviceChat"] = true
				}
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			access, uid := str(auth["access"]), str(auth["uid"])
			if access == "" || uid == "" {
				return ls, "", errors.New(qoder.SiteOf(id).Name + ": an unreadable plugin sign-in")
			}
			if user == "" {
				user = firstNonEmpty(str(auth["accountId"]), str(auth["email"]), uid)
			}
			i := backInto(&ls, id, user)
			c, _ := qoderSaved(ls[i])
			c.UID, c.Token, c.RefreshToken, c.ExpiresAt = uid, access, str(auth["refresh"]), num(auth["expires"])
			c.Email = firstNonEmpty(str(auth["email"]), c.Email)
			c.Name = firstNonEmpty(str(auth["name"]), c.Name)
			c.MachineID = firstNonEmpty(str(auth["machineId"]), c.MachineID)
			c.DeviceToken = firstNonEmpty(str(auth["deviceToken"]), c.DeviceToken)
			c.DeviceRefresh = firstNonEmpty(str(auth["deviceRefresh"]), c.DeviceRefresh)
			c.DeviceChat = auth["deviceChat"] == true
			b, err := json.Marshal(c)
			if err != nil {
				return ls, "", err
			}
			ls[i].Auth, ls[i].Lapsed, ls[i].Seen = b, "", time.Now().UTC().Truncate(time.Second)
			return ls, ls[i].User, nil
		},
	}
}

// num is a JSON number read into any (a float64, or an int64 before it
// was written out) as an int64.
func num(v any) int64 {
	switch n := v.(type) {
	case float64:
		return int64(n)
	case int64:
		return n
	case int:
		return int64(n)
	case json.Number:
		i, _ := n.Int64()
		return i
	}
	return 0
}
