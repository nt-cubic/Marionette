package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// #1904 (CnsMaple): an enterprise account's resource summary has no
// packages, and its credits are the enterprise meter's, which WorkBuddy's
// own usage page reads. The two answers are the reporter's, byte for byte,
// from one token at one moment.
const wbEntSummary = `{
  "code": 0,
  "msg": "OK",
  "data": {
    "Packages": [],
    "SubscriptionPackageCode": "",
    "SubscriptionPackageName": "",
    "IsPaidUser": false,
    "IsProtectedPriceUser": false
  }
}`

const wbEntUsage = `{
  "code": 0,
  "msg": "OK",
  "data": {
    "credit": 196.13,
    "cycleStartTime": "2026-10-08 00:00:00",
    "cycleEndTime": "2026-11-08 23:59:59",
    "limitNum": 2000,
    "cycleResetTime": "2026-11-09 00:00:00"
  }
}`

// and a personal account's answer to the enterprise meter, from a real
// WorkBuddy AI Pro account on 2026-10-13
const wbPersonalEnt = `{"code":10001,"msg":"uid or enterpriseID is empty","requestId":"36dd0f64-cb93-41e1-a964-9e3c87fea235"}`

func wbEntServer(t *testing.T, ent func(w http.ResponseWriter)) *[]string {
	t.Helper()
	var paths []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		switch r.URL.Path {
		case "/billing/meter/get-user-resource-summary":
			w.Write([]byte(wbEntSummary))
		case "/billing/meter/get-enterprise-user-usage":
			ent(w)
		default:
			w.WriteHeader(404)
		}
	}))
	t.Cleanup(srv.Close)
	old := wbEndpoint
	wbEndpoint = srv.URL
	t.Cleanup(func() { wbEndpoint = old })
	return &paths
}

func wbEntAccount(uid string) wbAccount {
	return wbAccount{Login: Login{User: "ent"}, site: wbCN, creds: wbCreds{Access: "ent-access", UID: uid, ExpiresAt: time.Now().Add(24 * time.Hour).UnixMilli()}}
}

func TestWorkBuddyEnterpriseCredits(t *testing.T) {
	wbEntServer(t, func(w http.ResponseWriter) { w.Write([]byte(wbEntUsage)) })
	q := wbQuota(context.Background(), wbEntAccount("ent-u1"))
	if q.Error != "" || q.Plan != "Enterprise" || len(q.Windows) != 1 {
		t.Fatalf("an enterprise account's card: %+v", q)
	}
	w := q.Windows[0]
	reset := time.Date(2026, 11, 8, 16, 0, 0, 0, time.UTC)
	if w.Name != "Credits" || w.Display != "196.13 / 2000" || w.Amount != 196.13 || w.Limit != 2000 || w.Unit != "credits" ||
		w.Used != 100*196.13/2000 || w.ResetsAt == nil || !w.ResetsAt.Equal(reset) || w.Span != 32*24*time.Hour {
		t.Fatalf("window: %+v (resets %v)", w, w.ResetsAt)
	}
}

// A personal account with no credits is refused the enterprise meter: its
// card stays as the summary said, with no error.
func TestWorkBuddyPersonalNotEnterprise(t *testing.T) {
	paths := wbEntServer(t, func(w http.ResponseWriter) {
		w.WriteHeader(400)
		w.Write([]byte(wbPersonalEnt))
	})
	q := wbQuota(context.Background(), wbEntAccount("ent-u2"))
	if q.Error != "" || q.Plan != "Free" || len(q.Windows) != 0 || len(*paths) != 2 {
		t.Fatalf("a personal account's card: %+v, asked %v", q, *paths)
	}
}
