package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

// Settings → Privacy reads what masking swapped: the summary, the records
// newest first, one record, and a value only while the masker remembers
// it; a placeholder it never made says nothing.
func TestMaskingRecordsAPI(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	defer gwmw.Mask(masking.Masker{})()
	m, _ := gwmw.Served()
	sent, hits := m.(gwmw.Teller).TextHits("my key is " + key)
	if len(hits) != 1 || strings.Contains(sent, key) {
		t.Fatalf("masked %q %+v", sent, hits)
	}
	p := hits[0].Placeholder
	rec := maskrec.Add(maskrec.Record{What: "chat", Agent: "claude", Provider: "anthropic", N: 1,
		Values: []maskrec.Value{{Placeholder: p, Kind: hits[0].Kind, By: hits[0].By, Head: hits[0].Head, Len: hits[0].Len}},
		Snips:  map[string]string{p: sent}})
	other := maskrec.Add(maskrec.Record{What: "search", N: 1, Values: []maskrec.Value{{Placeholder: "{{EMAIL_00000000}}", Kind: "EMAIL", By: redact.ByRule}}})

	srv := Handler(webHost{func() {}}, nil)
	get := func(path string, into any) int {
		rw := httptest.NewRecorder()
		srv.ServeHTTP(rw, httptest.NewRequest("GET", path, nil))
		if into != nil && rw.Code == http.StatusOK {
			if err := json.Unmarshal(rw.Body.Bytes(), into); err != nil {
				t.Fatalf("%s: %v %s", path, err, rw.Body)
			}
		}
		return rw.Code
	}
	var sum maskrec.Summary
	if get("/api/masking/summary?days=7", &sum); sum.Requests < 2 || len(sum.Days) != 7 {
		t.Fatalf("summary %+v", sum)
	}
	var page struct {
		Records []maskedRecord `json:"records"`
		More    bool           `json:"more"`
	}
	page.Records, page.More = nil, false
	if get("/api/masking/records?limit=1", &page); len(page.Records) != 1 || !page.More || page.Records[0].Key != other.Key {
		t.Fatalf("first page %+v", page)
	}
	page.Records, page.More = nil, false
	if get("/api/masking/records?limit=5&before="+strconv.FormatInt(other.Key, 10), &page); len(page.Records) == 0 || page.Records[0].Key != rec.Key || !page.Records[0].Values[0].Known {
		t.Fatalf("next page %+v", page)
	}
	page.Records, page.More = nil, false
	if get("/api/masking/records?kind=EMAIL", &page); len(page.Records) == 0 || page.Records[0].Key != other.Key || page.Records[0].Values[0].Known {
		t.Fatalf("by kind %+v", page)
	}
	var one maskedRecord
	if get("/api/masking/record?key="+strconv.FormatInt(rec.Key, 10), &one); one.Snips[p] != sent || !one.Values[0].Known {
		t.Fatalf("record %+v", one)
	}
	if code := get("/api/masking/record?key=1", nil); code != http.StatusNotFound {
		t.Fatalf("no such record: %d", code)
	}
	var v struct{ Value string }
	if get("/api/masking/value?p="+url.QueryEscape(p), &v); v.Value != key {
		t.Fatalf("value %q", v.Value)
	}
	if code := get("/api/masking/value?p="+url.QueryEscape("{{EMAIL_00000000}}"), nil); code != http.StatusNotFound {
		t.Fatalf("a placeholder it never made: %d", code)
	}
}
