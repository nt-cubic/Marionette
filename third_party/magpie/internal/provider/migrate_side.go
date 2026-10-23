package provider

// Movers for the subscriptions whose agent keeps an account of its own
// (Devin, Grok, Command Code, Cursor): the plugin takes that one the way
// its own "CLI's sign-in" method does, reading or copying what the CLI
// keeps, and the built-in keeps reading it after a move back; magpie's
// further accounts go over as the plugin keeps a sign-in.

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"time"
)

func init() {
	movers["devin"] = &mover{
		pkg:    "@magpie-community/opencode-devin-auth",
		min:    "0.1.23", // a reply with only usage or metadata is refused before the stream starts, not passed on as an empty success (plugins #297); 0.1.22: organisations under one email are kept apart, each with its own quota (plugins #296); 0.1.21: a chat request past 8 KiB goes as a gzipped Connect frame, as the devin CLI sends it, where an uncompressed large one could be refused 413 (plugins #275); 0.1.20: a tool result's images go to Devin on the tool message that answers the call, as the built-in sent them; 0.1.19: Devin answers in JSON when the request asks for it (response_format into the instructions, as the built-in); 0.1.18: a tool whose parameters have a root anyOf, oneOf or allOf goes as a plain object (#1196), a branch that is a union itself folded too (#1271); a turn's parallel calls split over two assistant messages keep their results (#1275); a temperature or top_p of 0 goes as 1e-6, which Devin takes (plugins #50); a call id with ":" or "#" goes to the agent as one Claude Code takes, and back to Devin as it was (#1304); a week used up, its share left out of GetUserStatus, is 100% and holds the account until its reset (面条); SWE-2, which Devin's quota doesn't count (its Free cost tier), is left out of the day and week and listed free, so a used-up quota no longer holds it (太陽と向日葵); a tool schema with a list type, an array with no items, or properties or an enum with no type goes to a Gemini model in a shape it takes, not "There is an issue with this request" (7eleven); each model carries the price Devin lists for it, and SWE-2 stays free beside the -lightning variants Devin now files under it (太陽と向日葵, Usage showed no cost)
		agents: []string{"devin"},
		out: func() ([]Moving, error) {
			var out []Moving
			for _, l := range devinLogins() {
				key, server, err := DevinAuthAt(l.Home)
				if err != nil {
					continue
				}
				// the CLI's own account has its plan from the CLI, as the
				// built-in shows it, not saved on its row
				plan := l.Plan
				if l.Home == "" {
					if _, p, ok := devinIdentity(); ok && p != "" {
						plan = p
					}
				}
				md := map[string]any{"email": l.User}
				if plan != "" {
					md["plan"] = plan
				}
				if s := strings.TrimRight(server, "/"); s != "" && s != devinServer {
					md["server"] = s
				}
				if l.Home == "" {
					md["cli"] = true // the plugin reads the CLI's key again, as the built-in does
				}
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.Lapsed != "", Plan: plan, Own: l.Home == "",
					Auth: map[string]any{"type": "api", "key": key, "metadata": md}})
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			key := str(auth["key"])
			md, _ := auth["metadata"].(map[string]any)
			if key == "" {
				return ls, "", errors.New("Devin: an unreadable plugin sign-in")
			}
			if own, _, err := DevinAuthAt(""); err == nil && (own == key || md["cli"] == true) {
				return ls, ownUser(ls, "devin", str(md["email"])), nil
			}
			if user == "" {
				user = str(md["email"])
			}
			if user == "" {
				return ls, "", errors.New("Devin: a plugin sign-in with no account")
			}
			i := backInto(&ls, "devin", user)
			home := ls[i].Home
			if home == "" {
				// the home the account had before the move, which it still has
				home = devinHomeWith(key)
			}
			if k, _, err := DevinAuthAt(home); home == "" || err != nil || k != key {
				if home == "" {
					h, err := newDevinHome()
					if err != nil {
						return ls, "", err
					}
					home = h
				}
				if err := os.MkdirAll(filepath.Dir(devinCredentialsAt(home)), 0o700); err != nil {
					return ls, "", err
				}
				if err := writePrivate(devinCredentialsAt(home), devinCredentials(key, str(md["server"]), "", "")); err != nil {
					return ls, "", err
				}
			}
			ls[i].Home, ls[i].Lapsed, ls[i].Seen = home, "", time.Now().UTC().Truncate(time.Second)
			if p := str(md["plan"]); p != "" {
				ls[i].Plan = p
			}
			return ls, ls[i].User, nil
		},
	}

	// Grok's plugin reads the CLI's auth.json in the home its sign-in
	// names, as the built-in does: the homes stay where they are.
	movers["grok"] = &mover{
		pkg:    "@magpie-community/opencode-grok-auth",
		min:    "0.2.1", // the sign-in page opens once, where Grok Build opened it and magpie again (andyfmyth on X); a Go key the Provider API refuses is taken as Go's and asked at /alpha/generate (#969); a failure's status and its sign-in mark as the built-in's; grok-4.7's reasoning levels; the sign-in renewed ahead of time through auth.refresh; a 429 shows a spent Rate limit window until it lifts; a login with no link says why; a tool whose parameters are a root union goes as a plain object (#1271); a plain Codex agent_message (a subagent's task or reply) goes as the user's message, not refused with 422 "unknown item type", a sealed one as it came (plugins#61); a model Grok lists at two windows is offered at each, grok-4.7@500k sent with x-grok-context-window (plugins#83); a window offered without a k (grok-4.7@131072) goes as that window, not as a model named so (#1679); a sign-in without Grok Build names GROK_BIN_DIR (#2223); a sign-in with no email (an X account) is kept by its xAI user id, GROK_AUTH_PATH no longer takes it elsewhere, and a login that leaves none says where it looked; a tool_choice with no tools beside it, which magpie hands the plugin as "none" when a client turned tools off (#3110), is left out before Grok; a 401 renews once through Grok Build and retries, a renewal that leaves the same token says Grok Build couldn't renew it, not that the sign-in expired, and a half-written auth.json is read again (plugins#324)
		agents: []string{"grok"},
		out: func() ([]Moving, error) {
			var out []Moving
			for _, l := range grokLogins() {
				c, ok := readGrokCredential(l.Home)
				if !ok {
					continue
				}
				var exp int64
				if !c.ExpiresAt.IsZero() {
					exp = c.ExpiresAt.UnixMilli()
				}
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.Lapsed != "", Plan: l.Plan, Own: samePath(l.Home, GrokHome()),
					Auth: map[string]any{"type": "oauth", "refresh": l.Home, "access": c.Key, "expires": exp, "accountId": firstNonEmpty(c.Email, l.User)}})
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			home := str(auth["refresh"])
			if home == "" {
				return ls, "", errors.New("Grok: an unreadable plugin sign-in")
			}
			if samePath(home, GrokHome()) {
				return ls, ownUser(ls, "grok", str(auth["accountId"])), nil
			}
			if c, ok := readGrokCredential(home); ok && c.Email != "" {
				user = c.Email
			} else if user == "" {
				user = str(auth["accountId"])
			}
			if user == "" {
				return ls, "", errors.New("Grok: a plugin sign-in with no account")
			}
			i := backInto(&ls, "grok", user)
			ls[i].Home, ls[i].Lapsed, ls[i].Seen = home, "", time.Now().UTC().Truncate(time.Second)
			return ls, ls[i].User, nil
		},
	}

	movers[CommandCodePlanID] = &mover{
		pkg:    "@magpie-community/opencode-commandcode-auth",
		min:    "0.1.15", // DeepSeek V4 Pro, V4 Flash, V4 Flash Vision, V4.1 Flash and V4.1 Flash Fast offer an off level, sent to the Provider API as reasoning_effort "off" (plugins #334); 0.1.14: a Zero Data Retention switch (or CMD_ZDR=1) sends x-cmd-zdr: 1 on the Provider API and /alpha/generate, as the CLI does (plugins #246); 0.1.13: the Credits window resets at the plan's period end (#2454); the models the built-in counted free are marked free; max_tokens capped at 200,000 as the built-in; 0.1.12: each plan lists only the models its plan may use, as the CLI's table has it, so a Go account no longer lists GOAT's (plugins 49e9d89, 01huadalang on Discord); 0.1.11: a streamed Go reply the upstream cuts off after it has said something ends cut short with a note, not a lost turn (plugins #91); 0.1.10: a Go request says the agent's folder and git repository as magpie tells them, not the home's (plugins #86); 0.1.9: a Go key the Provider API refuses is taken as Go's and asked at /alpha/generate (#969); a failure's status and its sign-in mark as the built-in's; a Go account lists Go's models; a model the plugin doesn't know takes pictures as models.dev says
		agents: []string{CommandCodePlanID},
		out: func() ([]Moving, error) {
			var out []Moving
			for _, l := range cmdLogins() {
				md := map[string]any{"email": l.User}
				if l.auth.UserID != "" {
					md["userId"] = l.auth.UserID
				}
				if l.auth.KeyName != "" {
					md["keyName"] = l.auth.KeyName
				}
				if l.Plan != "" {
					md["plan"] = l.Plan
				}
				if l.Own {
					md["cli"] = true // the plugin reads the CLI's key again, as the built-in does
				}
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.Lapsed != "", Plan: l.Plan, Own: l.Own,
					Auth: map[string]any{"type": "api", "key": l.auth.APIKey, "metadata": md}})
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			key := str(auth["key"])
			md, _ := auth["metadata"].(map[string]any)
			if key == "" {
				return ls, "", errors.New("Command Code: an unreadable plugin sign-in")
			}
			if _, own, ok := cmdOwn(); ok && (own.APIKey == key || md["cli"] == true) {
				return ls, ownUser(ls, CommandCodePlanID, str(md["email"])), nil
			}
			i := -1
			// the same key signed in under another name is the same account
			for j, l := range ls {
				if a, ok := cmdSaved(l); ok && l.Agent == CommandCodePlanID && a.APIKey == key {
					i = j
				}
			}
			if i < 0 {
				if user == "" {
					user = str(md["email"])
				}
				if user == "" {
					return ls, "", errors.New("Command Code: a plugin sign-in with no account")
				}
				i = backInto(&ls, CommandCodePlanID, user)
			}
			a, _ := cmdSaved(ls[i])
			a.APIKey = key
			if v := str(md["userId"]); v != "" {
				a.UserID = v
			}
			if v := str(md["keyName"]); v != "" {
				a.KeyName = v
			}
			if a.UserName == "" && a.UserID != ls[i].User {
				a.UserName = ls[i].User
			}
			ls[i].Auth, ls[i].Lapsed, ls[i].Seen = []byte(jsonText(a)), "", time.Now().UTC().Truncate(time.Second)
			if p := str(md["plan"]); p != "" {
				ls[i].Plan = p
			}
			return ls, ls[i].User, nil
		},
	}

	// Cursor's one account is cursor-agent's own, which magpie never keeps:
	// the plugin reads cursor-agent's token as the built-in does.
	movers["cursor"] = &mover{
		pkg:    "@magpie-community/opencode-cursor-auth",
		min:    "0.2.19", // two different native tool counts in one step, before any output and with no result submitted, rebuild once from the history instead of failing as "inconsistent native tool count" (plugins #207); 0.2.18: a failed tool result reaches Cursor as failed on the stateless path too; the keychain token is read once per 10s and, after a refused read, not asked again for 10 minutes, as the built-in; 0.2.17: a tool-result continuation in a Run's last minute rebuilds from the supplied history before writing results, not a 502 "native Run lifetime expired" (plugins #157); 0.2.16: a multi-step native turn's last step reports its own share of the turn's usage, the rest billed as usage.magpie_deferred, so Codex doesn't read 1.1M as a 61K context and compact at once (plugins #156); a step's calls get the batch wait from the latest one, so a step of many long calls isn't failed as "did not complete its declared native tool batch" (plugins #155); 0.2.15: calls the plugin answers itself (a tool the request doesn't have, get_mcp_tools) count toward Cursor's tool count, so a step with one isn't failed as "inconsistent native tool count" or a batch never completed, and a long history's blob burst waits for a slow link instead of failing as "native outbound messages limit reached" (plugins #142); 0.2.14: a Run tool call stays on one native Cursor session across its result, and a tool that outlasts Cursor resumes stateless, not as a failed turn (plugins #135); 0.2.13: a Run cancelled while it opens is closed upstream, not left streaming for nobody (plugins #130), and dashboard usage is collected per account, so one account never takes another's event as its debt (plugins #131); 0.2.12: Grok Bot is among the panel's three rings ahead of an On-demand with no limit, which is only a dollar figure (#2213); 0.2.11: a step's usage is its own, and what it collected of earlier steps goes as usage.magpie_deferred, billed with it but never told as its prompt, so pi's context isn't inflated (#1053, plugins #101); 0.2.10: a Team member with no limit of their own spends the team's on-demand pool, which is shown and counted as the limit, not as unlimited (#1868); 0.2.9: a default model whose own branch stops short of the asked effort goes to its nearest effort, so the id and its parameters agree (plugins #55); 0.2.8: a failure's status and its sign-in mark as the built-in's; Max Mode models retried in Max Mode; glm-5.3 listed; catalog entries written as CallDynamicTool calls, an empty turn retried (plugins #11); a tool step's usage from the dashboard's usage event, cache included (#676); far from Cursor a step whose event shows late is counted by the conversation's next steps instead of guessed (#1053); a turn's parallel calls split over two assistant messages keep their results (#1275); the list is Cursor's picker, a model once per context size with that window, effort and fast sent as the variant's parameters, and an id 0.1.x listed still runs as the variant it stood for (plugins #51); an id 0.1.x listed, at an effort only its other branch has, goes as that branch's nearest effort, not as the default's id with parameters that contradict it (plugins #54); a model listed at a size Cursor has a fast variant of says fast, for magpie's Fast switch (#1360); a Run HTTP/2 can't open (a proxy or network that blocks it, "h2 is not supported") goes over HTTP/1.1 as RunSSE + BidiAppend on api2 (plugins #59); a model asked for by its bare name (grok-4.7) counts in the pool its sized ids (grok-4.7@256k) are in (Will on Discord); an account with a Grok Bot allowance shows it on its Usage card, aside, so a spent Bot never stops Cursor requests (plugins #64); an account that may spend on-demand (a team or enterprise that pays past the included usage) is not used up when its included pools are, its On-demand spend is (fottencity on Discord, #1476); an assistant turn's reasoning_content is kept across a tool continuation (plugins #82); an empty reply says what Cursor's Run sent before it, so the next report shows why (plugins #60)
		agents: []string{"cursor"},
		out: func() ([]Moving, error) {
			if CursorExecutable() == "" || cursorSignedOut() {
				return nil, nil
			}
			user, plan, ok := cursorIdentity()
			if !ok || user == "" {
				return nil, nil
			}
			a := map[string]any{"type": "oauth", "access": "", "refresh": cursorCLIMark, "expires": 0, "accountId": user}
			if plan != "" {
				a["plan"] = plan
			}
			return []Moving{{User: user, First: true, On: true, Own: true, Auth: a}}, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			if str(auth["refresh"]) == cursorCLIMark {
				return ls, firstNonEmpty(user, str(auth["accountId"])), nil
			}
			// the built-in has nowhere to keep a token of its own
			// signed in in the browser through the plugin: the built-in
			// Cursor has nowhere to keep it
			return ls, "", errStays
		},
	}
}

const (
	devinServer   = "https://server.codeium.com"
	cursorCLIMark = "cursor-agent" // the Cursor plugin's sign-in that reads cursor-agent's token
)

// ownUser is the account the agent's own sign-in is saved as, else user.
func ownUser(ls []savedLogin, agent, user string) string {
	for _, l := range ls {
		if l.Agent == agent && l.own() && l.User != "" {
			return l.User
		}
	}
	return user
}

func samePath(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	return filepath.Clean(a) == filepath.Clean(b)
}

// devinHomeWith is the home of magpie's signed in with key, "" for none.
func devinHomeWith(key string) string {
	es, _ := os.ReadDir(devinAccountsDir())
	for _, e := range es {
		home := filepath.Join(devinAccountsDir(), e.Name())
		if k, _, err := DevinAuthAt(home); e.IsDir() && err == nil && k == key {
			return home
		}
	}
	return ""
}
