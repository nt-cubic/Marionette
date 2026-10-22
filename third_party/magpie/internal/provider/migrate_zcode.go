package provider

import (
	"encoding/json"
	"errors"
	"strings"
	"time"
)

// ZCode's accounts on @magpie-community/opencode-zcode-auth: the plan's
// key (else ZCode's session token) as the oauth entry's access, the rest
// the requests need as its refresh, as the plugin's own sign-in keeps
// them. Nothing here rotates: a key lasts until it is deleted.
//
// ZCode's own sign-in goes marked as ZCode's (source "zcode"), with a copy
// of what magpie reads now: the plugin, 0.1.2 on (the move installs no
// older), reads ZCode's own credentials in its place for each request, as
// the built-in does, so it follows ZCode's switches and the session token
// ZCode renews (the Start Plan).
func init() {
	movers["zcode"] = &mover{
		pkg:    "@magpie-community/opencode-zcode-auth",
		min:    "0.1.20", // a team's GLM Coding Plan signs in beside the account's own, as an account of its own named "<email> (team)" (plugins #285); 0.1.19: the personal plan's resets, and the team's, carry each reset's end and window; a Start Plan refusal carries the built-in's hint; 0.1.18: the ZCode app's sign-in reads ZCode 3.15's GLM Coding Plan key from config.json (provider["builtin:*-coding-plan"].options.apiKey), so a valid plan no longer reads as "no plan", as the built-in does since 1d8e83a79 (#2130); 0.1.17: the ZCode app's sign-in reads credentials.json from the folder ZCode's data was moved to in its settings (数据存储路径, dataBaseDir in the home folder's setting.json), as the built-in does since 41972b25f (flower on Discord); 0.1.16's ZCode app's sign-in follows ZCode only while it is signed in to the same account, so an entry added for one account never sends with another's credentials after ZCode switched (LingYi_Stu on X); 0.1.15's Start Plan request says the agent's folder and git repository as magpie tells them, never magpie's own folder, and leaves out what it doesn't know (plugins #86); 0.1.14's gift-only account lists its gift's models by their own names, as ZCode does, so a Start Plan GLM-5.3 bucket's model is listed as GLM-5.3, not only GLM-5.3-Trial (#1261, plugins aca7c59); 0.1.13's claim line has one fixed name, which the GUI translates, so it no longer shares the "ZCode Trust Build" bucket window's name (plugins #43); 0.1.12's counted window carries its amount and limit, so the card's count follows used/left (#659, plugins #35); 0.1.11's gift plan ZCode holds but hasn't granted is named on the card, to claim in the ZCode app (plugins #30, #1001); 0.1.10's gift-only buckets and 0.1.9's trial entries (plugins #21) kept
		agents: []string{"zcode"},
		out: func() ([]Moving, error) {
			var out []Moving
			for _, l := range zcodeLogins() {
				// the own account's copy goes back to nothing: ZCode keeps it
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.Lapsed != "", Plan: l.Plan, Own: l.Own, Auth: zcodeOut(l.User, l.Plan, l.key, l.Own)})
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			var s struct {
				Site, Device, Key, Base, JWT, Token, Org, Project, Plan, Source string
			}
			if json.Unmarshal([]byte(str(auth["refresh"])), &s) != nil {
				// a key signed in to in the plugin
				if str(auth["type"]) != "api" || str(auth["key"]) == "" {
					return ls, "", errors.New("ZCode: an unreadable plugin sign-in")
				}
				s.Key = str(auth["key"])
				if m, _ := auth["metadata"].(map[string]any); m != nil {
					s.Site = str(m["site"])
				}
			} else if s.Key == "" && s.JWT == "" {
				s.Key = str(auth["access"])
			}
			// ZCode's own goes back to nothing: ZCode keeps it
			if _, _, ok := zcodeOwn(); ok && s.Source == "zcode" {
				return ls, ownUser(ls, "zcode", firstNonEmpty(user, str(auth["accountId"]))), nil
			}
			k := zcodeKey{Key: s.Key, Base: s.Base, JWT: s.JWT, Token: s.Token, Org: s.Org, Project: s.Project}
			if k.Base == "" {
				k.Base = ZCodeZaiBase
				if s.Site == "bigmodel" {
					k.Base = ZCodeBigModelBase
				}
			}
			if k.Key == "" && k.JWT == "" && !k.team() {
				return ls, "", errors.New("ZCode: an unreadable plugin sign-in")
			}
			if user == "" {
				user = firstNonEmpty(str(auth["accountId"]), "ZCode")
			}
			i := backInto(&ls, "zcode", user)
			b, err := json.Marshal(k)
			if err != nil {
				return ls, "", err
			}
			ls[i].Auth, ls[i].Lapsed, ls[i].Seen = b, "", time.Now().UTC().Truncate(time.Second)
			if s.Plan != "" {
				ls[i].Plan = s.Plan
			}
			return ls, ls[i].User, nil
		},
	}
}

// zcodeOut is a ZCode account as the plugin's sign-in keeps one;
// ZCode's own is marked as ZCode's, for the plugin to follow.
func zcodeOut(user, plan string, k zcodeKey, own bool) map[string]any {
	site := "zai"
	if strings.Contains(k.Base, "bigmodel.cn") {
		site = "bigmodel"
	}
	state := map[string]any{"site": site, "device": zcodeDeviceMid(), "base": k.Base}
	if own {
		state["source"] = "zcode"
	}
	for name, v := range map[string]string{"key": k.Key, "jwt": k.JWT, "token": k.Token, "org": k.Org, "project": k.Project, "plan": plan} {
		if v != "" {
			state[name] = v
		}
	}
	expires := int64(0)
	if k.Key == "" && !k.team() && !own { // ZCode's own is read anew each time
		if exp, _ := jwtClaims(k.JWT)["exp"].(float64); exp > 0 {
			expires = int64(exp) * 1000
		}
	}
	return map[string]any{"type": "oauth", "access": firstNonEmpty(k.Key, k.JWT), "refresh": jsonText(state), "expires": expires, "accountId": user}
}
