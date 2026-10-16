package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// zcodeTeamUpstream is zcode.z.ai and BigModel (bigmodel.cn and
// open.bigmodel.cn) for these tests: a BigModel account with a personal
// project that has no plan, and two team coding plan projects, tp0 where
// it has no seat and tp1 where its seat is grant.
type zcodeTeamUpstream struct {
	srv   *httptest.Server
	grant string // tp1's memberGrantStatus
	mu    sync.Mutex
	made  map[string]any // the team key made, as it was asked for
	model *http.Request  // the last request to the plan's endpoint
	quota *http.Request  // the last quota asked
}

const zcodeTeamToken = "bm-tok" // BigModel's sign-in token, as the poll hands it back

func newZCodeTeamUpstream(t *testing.T) *zcodeTeamUpstream {
	t.Helper()
	u := &zcodeTeamUpstream{grant: "VALID"}
	u.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		org, proj := r.Header.Get("Bigmodel-Organization"), r.Header.Get("Bigmodel-Project")
		ok := func(data any) {
			json.NewEncoder(w).Encode(map[string]any{"code": 200, "msg": "ok", "data": data, "success": true})
		}
		deny := func() { w.WriteHeader(401) }
		switch p := r.URL.Path; {
		case p == "/api/v1/oauth/cli/init":
			var body map[string]string
			json.NewDecoder(r.Body).Decode(&body)
			if body["provider"] != "bigmodel" {
				w.WriteHeader(400)
				return
			}
			ok(map[string]any{"flow_id": "f1", "authorize_url": "https://open.bigmodel.cn/oauth?client_id=c",
				"expires_at": time.Now().Add(time.Minute).Unix(), "poll_interval_sec": 0})
		case p == "/api/v1/oauth/cli/poll/f1":
			ok(map[string]any{"status": "ready", "token": "zc", "bigmodel": map[string]any{"accessToken": zcodeTeamToken},
				"user": map[string]any{"user_id": "u9", "email": "team@example.com", "name": "Team"}})
		case p == "/api/auth/z/login":
			deny() // BigModel's token is never exchanged
		case p == "/api/biz/customer/getCustomerInfo":
			if auth != zcodeTeamToken {
				deny()
				return
			}
			ok(map[string]any{"organizations": []any{
				map[string]any{"organizationId": "o1", "organizationName": "默认机构", "projects": []any{
					map[string]any{"projectId": "p1", "projectName": "默认项目", "projectType": "1"}}},
				map[string]any{"organizationId": "t1", "organizationName": "Acme", "projects": []any{
					map[string]any{"projectId": "tp0", "projectName": "old team", "projectType": 2},
					map[string]any{"projectId": "tp1", "projectName": "team", "projectType": "2"}}},
			}})
		case p == "/api/biz/v1/organization/o1/projects/p1/api_keys" && auth == zcodeTeamToken:
			ok([]any{map[string]any{"name": "zcode-api-key", "apiKey": "pk"}})
		case p == "/api/biz/v1/organization/o1/projects/p1/api_keys/copy/pk" && auth == zcodeTeamToken:
			ok(map[string]any{"secretKey": "ps"})
		case p == "/api/biz/subscription/list":
			if auth != "pk.ps" {
				deny()
				return
			}
			ok([]any{}) // no plan of its own
		case p == "/api/biz/team/subscribe/product/querySubscribeDetail":
			if auth != zcodeTeamToken || org != "t1" {
				deny()
				return
			}
			switch proj {
			case "tp0":
				ok(map[string]any{"hasSubscription": true, "status": "EFFECTIVE", "memberGrantStatus": "UNASSIGNED", "productName": "GLM Coding Team Lite"})
			case "tp1":
				ok(map[string]any{"hasSubscription": true, "status": "EFFECTIVE", "memberGrantStatus": u.grant, "productId": "prod-1",
					"productName": "GLM Coding Team Pro", "subscribeEndTime": "2026-12-31 23:59:59"})
			default:
				deny()
			}
		case p == "/api/biz/v1/organization/t1/projects/tp1/api_keys":
			if auth != zcodeTeamToken || org != "t1" || proj != "tp1" {
				deny()
				return
			}
			u.mu.Lock()
			defer u.mu.Unlock()
			if r.Method == http.MethodPost {
				json.NewDecoder(r.Body).Decode(&u.made)
				ok(map[string]any{"name": "zcode-team-api-key", "apiKey": "tk", "keyType": 2})
				return
			}
			list := []any{map[string]any{"name": "zcode-team-api-key", "apiKey": "wrong-type", "keyType": 1}}
			if u.made != nil {
				list = append(list, map[string]any{"name": "zcode-team-api-key", "apiKey": "tk", "keyType": 2})
			}
			ok(list)
		case p == "/api/biz/v1/organization/t1/projects/tp1/api_keys/copy/tk":
			if auth != zcodeTeamToken || org != "t1" || proj != "tp1" {
				deny()
				return
			}
			ok(map[string]any{"secretKey": "ts"})
		case p == "/api/monitor/usage/quota/limit":
			u.mu.Lock()
			u.quota = r
			u.mu.Unlock()
			if auth != "tk.ts" || r.URL.Query().Get("type") != "2" {
				ok(map[string]any{"limits": []any{}})
				return
			}
			reset := time.Now().Add(2 * time.Hour).UnixMilli()
			ok(map[string]any{"limits": []any{
				map[string]any{"type": "CREDIT_LIMIT", "unit": 3, "percentage": 42, "currentValue": 420, "usage": 1000, "nextResetTime": reset},
				map[string]any{"type": "CREDIT_LIMIT", "unit": 6, "percentage": 10, "currentValue": 1000, "usage": 10000, "nextResetTime": reset + 86400000},
			}})
		case p == "/api/biz/customer-package-reset/list":
			if auth != zcodeTeamToken || org != "t1" || proj != "tp1" || r.URL.Query().Get("targetType") != "TEAM" {
				deny()
				return
			}
			ok(map[string]any{
				"fiveHourResets": []any{
					map[string]any{"available": true, "recordId": 1, "expireTime": "2026-10-05 00:00:00", "grantType": "MONTHLY"},
					map[string]any{"available": false, "recordId": 2, "expireTime": "2026-10-01 00:00:00"},
					map[string]any{"available": true, "recordId": 3, "expireTime": "2026-10-20 00:00:00"},
				},
				"weekResets": []any{map[string]any{"available": true, "recordId": 4, "expireTime": "2026-10-10 00:00:00"}},
			})
		case strings.HasSuffix(p, "/v1/messages"):
			u.mu.Lock()
			u.model = r
			u.mu.Unlock()
			w.Write([]byte(`{}`))
		default:
			w.WriteHeader(404)
		}
	}))
	t.Cleanup(u.srv.Close)
	oldAPI, oldZai, oldBM, oldZaiBase, oldBMBase := zcodeAPI, zcodeZaiAPI, zcodeBigModelAPI, ZCodeZaiBase, ZCodeBigModelBase
	zcodeAPI, zcodeZaiAPI, zcodeBigModelAPI = u.srv.URL, u.srv.URL, u.srv.URL
	ZCodeZaiBase, ZCodeBigModelBase = u.srv.URL+"/zai/api/anthropic", u.srv.URL+"/bigmodel/api/anthropic"
	t.Cleanup(func() {
		zcodeAPI, zcodeZaiAPI, zcodeBigModelAPI, ZCodeZaiBase, ZCodeBigModelBase = oldAPI, oldZai, oldBM, oldZaiBase, oldBMBase
	})
	zcodeRoutes.Lock()
	zcodeRoutes.m = map[string]zcodeRoute{}
	zcodeRoutes.Unlock()
	zcodeTeamKeys.Lock()
	zcodeTeamKeys.m = map[string]zcodeTeamKeyAt{}
	zcodeTeamKeys.Unlock()
	return u
}

// #236: an account on a BigModel team's GLM Coding Plan was refused ("no
// GLM Coding Plan, and ZCode's Start Plan has ended"), since magpie only
// signed in on Z.ai and only looked for a plan of the account's own. It
// signs in on BigModel now, finds its seat and the team project's key as
// ZCode does, and is served, metered and counted on the team's plan.
func TestZCodeTeamSignIn(t *testing.T) {
	signIn(t)
	t.Setenv("ZCODE_CREDENTIAL_SECRET", "test-secret")
	u := newZCodeTeamUpstream(t)

	st, err := StartSignIn("zcode:bigmodel")
	if err != nil {
		t.Fatal(err)
	}
	pu, _ := url.Parse(st.URL)
	if pu.Host != "open.bigmodel.cn" || !strings.Contains(pu.Query().Get("redirect"), "zcode%3A%2F%2Foauth%2Fcallback") || pu.Query().Get("redirect_uri") != "" {
		t.Fatalf("sign-in page: %s", st.URL)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	st, _ = WaitSignIn(ctx, st.ID)
	if st.State != "done" || st.User != "team@example.com" || st.Plan != "GLM Coding Team Pro" {
		t.Fatalf("done: %+v", st)
	}
	// the team key was made as ZCode makes it, the wrong type passed over
	if u.made["name"] != "zcode-team-api-key" || u.made["keyType"] != float64(2) {
		t.Fatalf("team key made: %v", u.made)
	}
	ls := zcodeLogins()
	if len(ls) != 1 || ls[0].key.Key != "tk.ts" || ls[0].key.Org != "t1" || ls[0].key.Project != "tp1" || ls[0].key.Token != zcodeTeamToken ||
		ls[0].key.Base != ZCodeBigModelBase || ls[0].key.JWT != "zc" {
		t.Fatalf("saved: %+v", ls)
	}

	// served at BigModel's coding plan endpoint with the team key, never the
	// Start Plan's, though it holds ZCode's token
	p, ok := find(All(), "zcode")
	if !ok || p.Anthropic != ZCodeBigModelBase {
		t.Fatalf("provider: %+v", p)
	}
	req, _ := http.NewRequest("POST", p.Anthropic+"/v1/messages", strings.NewReader(`{}`))
	req.Header.Set("Authorization", "Bearer magpie")
	if err := p.Sign(context.Background(), req, Anthropic, []byte(`{}`)); err != nil ||
		req.Header.Get("x-api-key") != "tk.ts" || req.Header.Get("Authorization") != "Bearer tk.ts" ||
		!strings.HasPrefix(req.URL.String(), ZCodeBigModelBase) || req.Header.Get("X-ZCode-App-Version") != "" {
		t.Fatalf("signed: %v %s %v", err, req.URL, req.Header)
	}
	if zcodeOnStart(context.Background(), ls[0].key) {
		t.Fatal("a team seat went to the Start Plan")
	}

	// its usage: the team's five hours and week, its resets, its plan's end
	q := LoginUsage(context.Background(), "zcode")["team@example.com"]
	if q.Error != "" || q.Plan != "GLM Coding Team Pro" || len(q.Windows) != 2 {
		t.Fatalf("usage: %+v", q)
	}
	five, week := q.Windows[0], q.Windows[1]
	if five.Name != "5 hours" || five.Used != 42 || five.Span != 5*time.Hour || five.ResetsAt == nil || five.Display != "420 / 1000" {
		t.Fatalf("five hours: %+v", five)
	}
	if week.Name != "Weekly" || week.Used != 10 || week.Span != 7*24*time.Hour {
		t.Fatalf("week: %+v", week)
	}
	qr := u.quota
	if qr.Header.Get("Bigmodel-Organization") != "t1" || qr.Header.Get("Bigmodel-Project") != "tp1" || qr.Header.Get("Accept-Language") == "" || qr.Header.Get("Set-Language") == "" {
		t.Fatalf("quota asked with: %v", qr.Header)
	}
	r := q.Resets
	if r == nil || !r.ByWindow || r.FiveHour != 2 || r.Weekly != 1 || r.Count != 3 || r.Until == nil || r.Until.Format("2006-01-02") != "2026-10-05" {
		t.Fatalf("resets: %+v", r)
	}
	if len(r.Each) != 3 || r.Each[0].Window != "fiveHour" || r.Each[0].Until.Format("2006-01-02") != "2026-10-05" ||
		r.Each[1].Window != "weekly" || r.Each[1].Until.Format("2006-01-02") != "2026-10-10" ||
		r.Each[2].Window != "fiveHour" || r.Each[2].Until.Format("2006-01-02") != "2026-10-20" {
		t.Fatalf("each reset: %+v", r.Each)
	}
	if got := r.Words(); got != "2 five-hour resets · 1 weekly reset" {
		t.Fatalf("resets in words: %q", got)
	}
	if q.Until == nil || q.Until.Format("2006-01-02") != "2026-12-31" || q.Renew != "off" {
		t.Fatalf("term: %v %q", q.Until, q.Renew)
	}
}

// A GLM key's team windows are asked where ZCode asks them, with the team
// project's headers when the key is a team seat's magpie holds.
func TestZhipuKeyTeamWindows(t *testing.T) {
	signIn(t)
	t.Setenv("ZCODE_CREDENTIAL_SECRET", "test-secret")
	u := newZCodeTeamUpstream(t)
	k, plan, err := zcodeSignedIn(context.Background(), "bigmodel", zcodeTeamToken, "")
	if err != nil || plan != "GLM Coding Team Pro" {
		t.Fatalf("signed in: %v %q", err, plan)
	}
	auth, _ := json.Marshal(k)
	if err := addSideLogin(savedLogin{Agent: "zcode", User: "team@example.com", Plan: plan, Auth: auth}, "", func(savedLogin) {}); err != nil {
		t.Fatal(err)
	}
	plan, ws, err := zhipuKeyTeamWindows(context.Background(), ZCodeBigModelBase, "tk.ts", nil)
	if err != nil || plan != "GLM Coding Team" || len(ws) != 2 || ws[0].Name != "5 hours" || ws[0].Used != 42 || ws[0].Span != 5*time.Hour || ws[1].Name != "7 days" {
		t.Fatalf("windows: %v %q %+v", err, plan, ws)
	}
	if u.quota.Header.Get("Bigmodel-Project") != "tp1" {
		t.Fatalf("asked with: %v", u.quota.Header)
	}
	// a key that isn't a seat magpie holds is asked without them, and has none
	if _, ws, err := zhipuKeyTeamWindows(context.Background(), ZCodeBigModelBase, "other.key", nil); err != nil || len(ws) != 0 {
		t.Fatalf("another key: %v %+v", err, ws)
	}
}

// A team member with no seat, and no plan or Start Plan besides, is told
// why rather than that there is no plan at all.
func TestZCodeTeamNoSeat(t *testing.T) {
	signIn(t)
	u := newZCodeTeamUpstream(t)
	u.grant = "UNASSIGNED"
	_, _, err := zcodeSignedIn(context.Background(), "bigmodel", zcodeTeamToken, "")
	if err == nil || !strings.Contains(err.Error(), "no seat") || !strings.Contains(err.Error(), "BigModel") {
		t.Fatalf("no seat: %v", err)
	}
	if u.made != nil {
		t.Fatalf("a key was made without a seat: %v", u.made)
	}
}

// ZCode's own account, switched in ZCode to a team's plan, keeps no team
// key: magpie finds it as ZCode does, from ZCode's sign-in and the project
// its settings name.
func TestZCodeOwnTeam(t *testing.T) {
	home := signIn(t)
	t.Setenv("ZCODE_CREDENTIAL_SECRET", "test-secret")
	u := newZCodeTeamUpstream(t)
	writeFile(t, filepath.Join(home, ".zcode", "v2", "credentials.json"), map[string]any{
		"oauth:bigmodel:user_info":    zcodeEncrypt(t, `{"user_id":"u9","email":"team@example.com"}`),
		"oauth:bigmodel:access_token": zcodeEncrypt(t, zcodeTeamToken),
		"zcodejwttoken":               zcodeEncrypt(t, "zc"),
	})
	writeFile(t, filepath.Join(home, ".zcode", "v2", "setting.json"), map[string]any{
		"providerFamilyDomain": "bigmodel",
		"providerFamilyConnectionSelections": map[string]any{
			"bigmodel": map[string]any{"kind": "team-coding-plan", "productId": "prod-1", "organizationId": "t1", "projectId": "tp1"},
		},
	})
	who, k, ok := zcodeOwn()
	if !ok || who != "team@example.com" || k.Key != "" || k.Org != "t1" || k.Project != "tp1" || k.Base != ZCodeBigModelBase || k.JWT != "zc" {
		t.Fatalf("own: %v %q %+v", ok, who, k)
	}
	p, ok := find(All(), "zcode")
	if !ok || p.Anthropic != ZCodeBigModelBase {
		t.Fatalf("provider: %+v", p)
	}
	req, _ := http.NewRequest("POST", p.Anthropic+"/v1/messages", strings.NewReader(`{}`))
	if err := p.Sign(context.Background(), req, Anthropic, []byte(`{}`)); err != nil || req.Header.Get("x-api-key") != "tk.ts" {
		t.Fatalf("signed: %v %v", err, req.Header)
	}
	if u.made == nil {
		t.Fatal("the team key wasn't found or made")
	}
	q := LoginUsage(context.Background(), "zcode")["team@example.com"]
	if q.Error != "" || len(q.Windows) != 2 || q.Windows[0].Used != 42 || q.Resets == nil || q.Resets.FiveHour != 2 {
		t.Fatalf("usage: %+v", q)
	}

	// ZCode on its individual plan again: as before
	writeFile(t, filepath.Join(home, ".zcode", "v2", "setting.json"), map[string]any{
		"providerFamilyDomain":               "bigmodel",
		"providerFamilyConnectionSelections": map[string]any{"bigmodel": map[string]any{"kind": "individual-coding-plan"}},
	})
	if _, k, ok := zcodeOwn(); !ok || k.team() {
		t.Fatalf("own, individual: %v %+v", ok, k)
	}
}
