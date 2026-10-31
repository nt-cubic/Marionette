package maskrec

import "sync"

// Which values a record has for the first time, for the notification that
// a value was masked (Must2333 on X): told at once, and only once. A
// conversation's history is sent again in full every turn, so a value
// masked in its first turn is masked again in every turn after; only the
// record that masked it first is news.
//
// first is each placeholder by the key of the first record that had it.
// A placeholder is the same for the same value across restarts (redact's
// key is kept on disk), so the index is read from the days kept, the last
// keepDays, as well as from memory: a restart doesn't make the history of
// a conversation new again. Clear leaves it as it is: the user who
// cleared the records was told of those values already.

var firstLoad sync.Mutex // one reading of the days at a time

// noteFirst puts r's values in the index; called with store.mu held.
func noteFirst(m map[string]int64, r Record) {
	for _, v := range r.Values {
		if k, ok := m[v.Placeholder]; !ok || r.Key < k {
			m[v.Placeholder] = r.Key
		}
	}
}

// loadFirst makes the index: what memory has at once, under the lock, then
// the days on disk read without it, since they can be many megabytes and
// Hold, on the request's way, takes the same lock. A record held while
// the days are read is put in the index by Hold, as store.first is there.
func loadFirst() {
	store.mu.Lock()
	store.first = map[string]int64{}
	for _, r := range store.snips {
		noteFirst(store.first, r)
	}
	dir := Dir()
	store.mu.Unlock()
	disk := map[string]int64{}
	if !inTest() {
		for _, d := range files(dir) {
			read(d.path, func(r Record) { noteFirst(disk, r) })
		}
	}
	store.mu.Lock()
	for p, k := range disk {
		if cur, ok := store.first[p]; !ok || k < cur {
			store.first[p] = k
		}
	}
	store.firstRead = true
	store.mu.Unlock()
}

// Fresh are the values r masked that no record before it had: masked for
// the first time. A value sent as written (Action "warn") is not one
// masked. r is a record as Hold returned it, its Key set.
func Fresh(r Record) []Value {
	firstLoad.Lock()
	store.mu.Lock()
	read := store.firstRead
	store.mu.Unlock()
	if !read {
		loadFirst()
	}
	firstLoad.Unlock()
	store.mu.Lock()
	defer store.mu.Unlock()
	var out []Value
	for _, v := range r.Values {
		if v.Action == "warn" {
			continue
		}
		if k, ok := store.first[v.Placeholder]; ok && k < r.Key {
			continue
		}
		out = append(out, v)
	}
	return out
}
