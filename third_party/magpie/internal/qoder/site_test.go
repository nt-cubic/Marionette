package qoder

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
	"time"
)

// TestSites: the global site is qoder.com as it always was, Qoder CN is
// qoder.cn with its CLI's client id and no redirect, and anything that isn't
// Qoder CN, a credential with no site included, is the global site.
func TestSites(t *testing.T) {
	for _, tt := range []struct {
		site                   *Site
		page, client, redirect string
		chat                   string
	}{
		{Global, "https://qoder.com", ClientID, RedirectURI, "https://api3.qoder.sh" + ChatPath},
		{CN, "https://qoder.cn", CNClientID, "", "https://gateway.qoder.com.cn" + ChatPath},
	} {
		authURL, _, _, err := NewDeviceFlow(nil, tt.site).Authorization()
		if err != nil {
			t.Fatal(err)
		}
		u, _ := url.Parse(authURL)
		if u.Scheme+"://"+u.Host != tt.page || u.Path != DeviceSelectAccountsPath || u.Query().Get("client_id") != tt.client || u.Query().Get("redirect_uri") != tt.redirect {
			t.Errorf("%s: %s", tt.site.ID, authURL)
		}
		if tt.site.ChatURL() != tt.chat {
			t.Errorf("%s chat %s", tt.site.ID, tt.site.ChatURL())
		}
	}
	if CN.OpenAPI != "https://openapi.qoder.com.cn" || SiteOf("qoder-cn") != CN || SiteOf("qoder") != Global || SiteOf("") != Global || ChatURL() != Global.ChatURL() {
		t.Fatal("site table")
	}
	var legacy Credential
	_ = json.Unmarshal([]byte(`{"uid":"u","token":"jt"}`), &legacy)
	if legacy.OnSite() != Global {
		t.Fatal("a credential without a site isn't global")
	}
}

// TestCNDeviceChatRefresh: a Qoder CN credential whose chat token is the
// device token refreshes on Qoder CN's device refresh, not the job one.
func TestCNDeviceChatRefresh(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != DeviceTokenRefreshPath {
			t.Errorf("path %s", r.URL.Path)
			w.WriteHeader(404)
			return
		}
		_, _ = w.Write([]byte(`{"token":"dt-2","refresh_token":"drt-2","expires_at":"` + time.Now().Add(3*time.Hour).UTC().Format(time.RFC3339) + `"}`))
	}))
	defer srv.Close()
	host := CN.OpenAPI
	CN.OpenAPI = srv.URL
	defer func() { CN.OpenAPI = host }()
	c := Credential{UID: "u", Token: "dt-1", RefreshToken: "drt-1", DeviceToken: "dt-1", DeviceRefresh: "drt-1", Site: CNProviderKey, DeviceChat: true}
	fresh, err := c.Refresh(context.Background(), srv.Client())
	if err != nil || fresh.Token != "dt-2" || fresh.DeviceToken != "dt-2" || fresh.RefreshToken != "drt-2" || fresh.DeviceRefresh != "drt-2" {
		t.Fatalf("%+v %v", fresh, err)
	}
	if left := time.Until(time.UnixMilli(fresh.ExpiresAt)); left < 2*time.Hour || left > 3*time.Hour {
		t.Fatalf("lifetime %v", left)
	}
}
