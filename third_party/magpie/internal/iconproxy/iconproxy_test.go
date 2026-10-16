package iconproxy

import (
	"net/url"
	"testing"
)

func TestURLCarriesThePictureWhole(t *testing.T) {
	t.Setenv("MAGPIE_ICON_PROXY", "")
	pic := "https://github.com/magpie-community.png?size=96&x=a b#c"
	got := URL(pic)
	u, err := url.Parse(got)
	if err != nil {
		t.Fatal(err)
	}
	if u.Scheme+"://"+u.Host+u.Path != Base {
		t.Fatalf("URL(%q) = %q, not on %s", pic, got, Base)
	}
	if q := u.Query(); len(q) != 1 || q.Get("url") != pic {
		t.Fatalf("URL(%q) = %q: query %v", pic, got, q)
	}
}

func TestURLLeavesUsemagpieAlone(t *testing.T) {
	t.Setenv("MAGPIE_ICON_PROXY", "")
	for u, want := range map[string]string{
		"https://usemagpie.ai/partners/x.png":     "https://usemagpie.ai/partners/x.png",
		"https://usemagpie.ai.evil.example/x.png": Base + "?url=" + url.QueryEscape("https://usemagpie.ai.evil.example/x.png"),
		"https://evil.example/?usemagpie.ai":      Base + "?url=" + url.QueryEscape("https://evil.example/?usemagpie.ai"),
	} {
		if got := URL(u); got != want {
			t.Errorf("URL(%q) = %q, want %q", u, got, want)
		}
	}
}
