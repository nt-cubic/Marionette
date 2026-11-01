package middleware

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/MoeForge/moejs"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/gwmw"
)

// account is what onAccount, and ctx.account after it, are told of the
// account a try goes to: never its email, login or id, only a ref.
type account struct {
	Ref      string `json:"ref"`
	Provider string `json:"provider"` // the provider's id
	Name     string `json:"name"`     // and its name
	Kind     string `json:"kind"`     // the subscription's kind: codex, claude, …
	Model    string `json:"model"`    // the model asked of it, by its id there
}

// Account runs each onAccount before a try goes to a, in turn, with
// a as an account (above) and ctx.account set to it; ctx.account stays for
// the hooks after it, onProvider, onEvent and onResponse, until the next
// try. The first that returns {skip: true} has the account passed over for
// this request: the hooks after it aren't asked, and the gateway goes on
// to the next account (skip, and by naming that middleware). A hook that
// throws, takes too long or returns what isn't an object or undefined
// skips nothing (fail open), counted as a failure. A try on a key (a.Agent
// "") asks no hook, and takes ctx.account away.
func (r *Run) Account(a gwmw.Account) (skip bool, by string) {
	if r == nil {
		return false, ""
	}
	defer r.letGo()
	for i, m := range r.set.list {
		if !m.hasAcct {
			continue
		}
		if a.Agent == "" {
			if in := r.insts[i]; in != nil {
				realm := in.rt.Realm()
				realm.SetV(r.ctxs[i], realm.KeyFromGoString("account"), moejs.Undefined())
			}
			continue
		}
		b, _ := json.Marshal(account{Ref: accountRef(m.name, a), Provider: a.ID, Name: a.Name, Kind: a.Agent, Model: a.Model})
		in, ctx, err := r.acquire(i)
		if err == nil {
			var v moejs.Value
			if v, err = in.rt.ParseJSON(b); err == nil {
				realm := in.rt.Realm()
				err = realm.SetV(ctx, realm.KeyFromGoString("account"), v)
			}
		}
		if err != nil {
			m.fail("onAccount", err)
			continue
		}
		out, err := r.apply(i, m.onAcct, requestLimit, b, nil)
		switch {
		case errors.Is(err, errDropped):
			m.fail("onAccount", errors.New("returned null; return {skip: true} to pass the account over"))
		case err != nil:
			m.fail("onAccount", err)
		case out != nil:
			var said struct {
				Skip bool `json:"skip"`
			}
			if json.Unmarshal(out, &said) == nil && said.Skip {
				return true, m.name
			}
		}
	}
	return false, ""
}

// accountRef is the account a, as middleware name is told it: a keyed hash
// of who the account is (its kind, user and plan), the same for every
// request and every start of magpie, different for each middleware, and
// one nobody can turn back into the email without this computer's key,
// even knowing the email to try.
func accountRef(name string, a gwmw.Account) string {
	mac := hmac.New(sha256.New, refKey())
	for _, s := range []string{"magpie account ref v1", name, a.Agent, strings.ToLower(a.User), strings.ToLower(a.Plan)} {
		mac.Write([]byte(s))
		mac.Write([]byte{0})
	}
	return hex.EncodeToString(mac.Sum(nil)[:16])
}

var refKeys = struct {
	sync.Mutex
	m map[string][]byte // by file
}{m: map[string][]byte{}}

// refKey is the key refs are made with: random, made the first time and
// kept in magpie's folder (account-ref.key), so a ref is the same after a
// restart. A key that can't be read is never written over, as every ref a
// middleware kept would change: this process makes do with one of its own
// and says so.
func refKey() []byte {
	p := filepath.Join(appdir.Config(), "account-ref.key")
	refKeys.Lock()
	defer refKeys.Unlock()
	if k, ok := refKeys.m[p]; ok {
		return k
	}
	k, err := readRefKey(p)
	if errors.Is(err, fs.ErrNotExist) {
		k, err = makeRefKey(p)
	}
	if err != nil {
		log.Printf("middleware: account refs are this run's only: %v", err)
		k = make([]byte, 32)
		rand.Read(k)
	}
	refKeys.m[p] = k
	return k
}

func readRefKey(p string) ([]byte, error) {
	b, err := os.ReadFile(p)
	if err != nil {
		return nil, err
	}
	k, err := hex.DecodeString(strings.TrimSpace(string(b)))
	if err != nil || len(k) != 32 {
		return nil, errors.New(p + " isn't a key")
	}
	return k, nil
}

// makeRefKey writes a new key to p, unless another process has just
// written one, which is then the key: written whole to a file of its own
// first and linked into place, which never writes over one there.
func makeRefKey(p string) ([]byte, error) {
	k := make([]byte, 32)
	if _, err := rand.Read(k); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		return nil, err
	}
	tmp, err := os.CreateTemp(filepath.Dir(p), ".account-ref-*")
	if err != nil {
		return nil, err
	}
	defer os.Remove(tmp.Name())
	_, err = tmp.WriteString(hex.EncodeToString(k) + "\n")
	if err == nil {
		err = tmp.Sync()
	}
	if cerr := tmp.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		return nil, err
	}
	if err := os.Link(tmp.Name(), p); err != nil {
		if errors.Is(err, fs.ErrExist) {
			// another process made it a moment ago, whole as this one's
			return readRefKey(p)
		}
		return nil, err
	}
	return k, nil
}
