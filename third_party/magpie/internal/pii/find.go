package pii

import (
	"container/list"
	"context"
	"crypto/sha256"
	"fmt"
	"sync"

	"github.com/yetone/magpie/internal/redact"
)

// Find is redact's Model: what the model finds in each of texts. A
// request's history comes back turn after turn, so what was found in a
// text is kept (by its hash, on disk too: cache.go) and the model is
// asked only about texts it hasn't seen. Those go to it joined into
// chunks of up to chunkSize: one pass over a few joined texts takes about
// what one over a single short text does (100 short messages: 683 ms one
// by one, 44 ms joined, on an M4 Pro's GPU).
func Find(texts []string) ([][]redact.Found, error) {
	return FindContext(context.Background(), texts)
}

// FindContext is Find for a caller that may stop waiting: when ctx ends
// (the client went away, the gateway gave up on the request) it returns
// at once, whether its turn hadn't come or pf-serve is on it. A turn
// begun is finished all the same and its answers kept, so the request
// sent again finds them.
func FindContext(ctx context.Context, texts []string) ([][]redact.Found, error) {
	loadCache()
	out, miss := cached(texts)
	if len(miss) == 0 {
		return out, nil
	}
	if ctx.Err() != nil {
		return nil, waitErr(ctx)
	}
	waiting.Add(1)
	select {
	case turn <- struct{}{}:
		waiting.Add(-1)
	case <-ctx.Done():
		waiting.Add(-1)
		return nil, waitErr(ctx)
	}
	type result struct {
		out [][]redact.Found
		err error
	}
	done := make(chan result, 1)
	go func() {
		defer func() { <-turn }()
		// read again: the requests before this one in turn had most of
		// a history in common with it, and asked already
		out, miss := cached(texts)
		if len(miss) > 0 {
			chunks, parts := join(texts, miss)
			got, err := classify(chunks)
			if err != nil {
				done <- result{nil, err}
				return
			}
			for c, ps := range parts {
				for _, p := range ps {
					out[p.text] = within(got[c], p.at, p.at+len(texts[p.text]))
				}
			}
			cache.mu.Lock()
			for _, i := range miss {
				cache.put(sha256.Sum256([]byte(texts[i])), out[i])
			}
			cache.mu.Unlock()
			saveSoon()
		}
		done <- result{out, nil}
	}()
	select {
	case r := <-done:
		return r.out, r.err
	case <-ctx.Done():
		return nil, waitErr(ctx)
	}
}

// waitErr is a request's that stopped waiting for the model.
func waitErr(ctx context.Context) error {
	return fmt.Errorf("stopped waiting for the local masking model: %w", context.Cause(ctx))
}

// cached is what the cache has of texts, and which it hasn't.
func cached(texts []string) (out [][]redact.Found, miss []int) {
	out = make([][]redact.Found, len(texts))
	cache.mu.Lock()
	defer cache.mu.Unlock()
	for i, t := range texts {
		if fs, ok := cache.get(sha256.Sum256([]byte(t))); ok && fits(fs, t) {
			out[i] = fs
		} else {
			miss = append(miss, i)
		}
	}
	return out, miss
}

// fits says every span of fs is inside t: one read from a damaged file
// that isn't is asked about again.
func fits(fs []redact.Found, t string) bool {
	for _, f := range fs {
		if f.Start < 0 || f.End > len(t) || f.Start >= f.End {
			return false
		}
	}
	return true
}

// chunkSize is how long a chunk of joined texts grows; a longer text
// goes alone, pf-serve reading it a window at a time.
const chunkSize = 12 << 10

// sep is put between joined texts: a paragraph's end, so the model reads
// each as a text of its own.
const sep = "\n\n"

type part struct{ text, at int }

// join puts texts[miss] into chunks, and says where in which each is.
func join(texts []string, miss []int) (chunks []string, parts [][]part) {
	cur, curParts := "", []part(nil)
	flush := func() {
		if curParts != nil {
			chunks, parts = append(chunks, cur), append(parts, curParts)
			cur, curParts = "", nil
		}
	}
	for _, i := range miss {
		t := texts[i]
		if curParts != nil && len(cur)+len(sep)+len(t) > chunkSize {
			flush()
		}
		if curParts != nil {
			cur += sep
		}
		curParts = append(curParts, part{i, len(cur)})
		cur += t
	}
	flush()
	return chunks, parts
}

// within are the spans of fs inside [a, b), made relative to a; one that
// runs past either end, over into the text beside it, is cut at the end.
func within(fs []redact.Found, a, b int) []redact.Found {
	var out []redact.Found
	for _, f := range fs {
		s, e := max(f.Start, a), min(f.End, b)
		if s < e {
			out = append(out, redact.Found{Start: s - a, End: e - a, Label: f.Label})
		}
	}
	return out
}

// cache keeps what was found in the texts seen last, by their hashes.
var cache = newLRU(20000)

type lru struct {
	mu  sync.Mutex
	max int
	ll  *list.List
	m   map[[32]byte]*list.Element
}

type entry struct {
	k  [32]byte
	fs []redact.Found
}

func newLRU(n int) *lru { return &lru{max: n, ll: list.New(), m: map[[32]byte]*list.Element{}} }

func (c *lru) get(k [32]byte) ([]redact.Found, bool) {
	e, ok := c.m[k]
	if !ok {
		return nil, false
	}
	c.ll.MoveToFront(e)
	return e.Value.(*entry).fs, true
}

func (c *lru) put(k [32]byte, fs []redact.Found) {
	if e, ok := c.m[k]; ok {
		c.ll.MoveToFront(e)
		e.Value.(*entry).fs = fs
		return
	}
	c.m[k] = c.ll.PushFront(&entry{k, fs})
	for c.ll.Len() > c.max {
		e := c.ll.Back()
		c.ll.Remove(e)
		delete(c.m, e.Value.(*entry).k)
	}
}
