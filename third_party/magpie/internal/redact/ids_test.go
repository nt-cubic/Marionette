package redact

import (
	"strings"
	"testing"
)

// An id goes as written wherever it is and whatever is around it: a UUID,
// one whose last group holds a phone number's digits, one whose groups read
// as a card number, without hyphens, in capitals, a commit's hash. It is
// masked whole only in a field named for a credential.
func TestIDsGoAsWritten(t *testing.T) {
	forgetIDs := func() {
		mu.Lock()
		values = map[string]string{}
		reindexKnown()
		mu.Unlock()
	}
	ids := []string{
		"3f2b8c1e-9a4d-4e7b-b2c1-7d6e5f4a3b21",
		"3F2B8C1E-9A4D-4E7B-B2C1-7D6E5F4A3B21",
		"3f2b8c1e-9a4d-4e7b-b2c1-a13800138000",         // PHONE in its last group
		"abcd4111-1111-1111-9007-abcdef123456",         // BANK_CARD across its groups
		"3f2b8c1e9a4d4e7bb2c1a13800138000",             // no hyphens
		"9f8e7d6cabcd4111111111111111900713800138000a", // a long hash with both
		"12345678-1234-1234-1234-123456789012",
	}
	kinds := map[string]bool{}
	for _, c := range Categories {
		kinds[c.ID] = true
	}
	o := Options{Secrets: true, Personal: true, Kinds: kinds}
	asWritten := []string{
		"%s", "id %s", `{"id":"%s"}`, "request_id: %s", "/users/%s/profile", "urn:uuid:%s", "{%s}",
		"next_page_token: %s", `"nextPageToken": "%s"`, `\"csrf_token\": \"%s\"`, `"token_id": "%s"`, "secret_id: %s",
		`"api_key_id": "%s"`, "access_key_id = %s", `"key": "%s"`, "Idempotency-Key: %s", "X-Request-Id: %s",
		"trace_token=%s", "secret_hash: %s", "sn: %s", "Serial Number: %s", "tel: %s",
	}
	for _, id := range ids {
		for _, c := range asWritten {
			forgetIDs()
			in := strings.ReplaceAll(c, "%s", id)
			if out, n := Mask(in, o); out != in || n != 0 {
				t.Errorf("%q went as %q", in, out)
			}
		}
	}
	credentials := []string{"password=%s", "api_key=%s", "X-Api-Key: %s", "token: %s", `"refresh_token": "%s"`, "session_token=%s", "postgres://app:%s@db:5432/x"}
	for _, id := range ids[:2] {
		for _, c := range credentials {
			forgetIDs()
			in := strings.ReplaceAll(c, "%s", id)
			if out, n := Mask(in, o); strings.Contains(out, id) || n != 1 {
				t.Errorf("a credential went out: %q as %q", in, out)
			}
		}
	}

	// the model takes an id for anything at all: it goes as written
	for label := range modelKinds {
		forgetIDs()
		id := ids[0]
		mo := o
		mo.Model = (&fakeModel{values: map[string]string{id: label}}).find
		body := `{"messages":[{"role":"user","content":"look at order ` + id + ` please"}]}`
		if got, n := MaskJSON([]byte(body), mo); string(got) != body || n != 0 {
			t.Errorf("the model's %s: %s", label, got)
		}
	}

	// a phone number masked before is not masked inside an id after
	forgetIDs()
	Mask("call 13800138000", o)
	if in := "order " + ids[2]; func() bool { out, _ := Mask(in, o); return out != in }() {
		t.Errorf("a known phone number was masked inside %q", ids[2])
	}

	// a phone number and a card number of their own are still masked
	for _, in := range []string{"call 13800138000", "card 4111-1111-1111-1111", "call a13800138000"} {
		forgetIDs()
		if out, n := Mask(in, o); n != 1 || out == in {
			t.Errorf("%q went as %q", in, out)
		}
	}

	// what the user chose stays theirs: their words, their rules
	forgetIDs()
	uo := o
	uo.Words = []string{ids[0]}
	if out, n := Mask("order "+ids[0], uo); strings.Contains(out, ids[0]) || n != 1 {
		t.Errorf("the user's word went out: %q", out)
	}
	forgetIDs()
}
