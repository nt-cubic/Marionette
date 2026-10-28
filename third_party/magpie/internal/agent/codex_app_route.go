package agent

import (
	"crypto/sha256"
	"encoding/hex"
	"os"
	"sync"
	"time"
)

// codexCatalogTag stands for what the catalog file at path holds now, for
// codexAppRoute: a hash of its bytes, "" when it can't be read. The route
// is looked at every few seconds (provider.KeepCodexAppOnItsProvider) and
// the catalog holds each model's instructions, a few MB with many models,
// so the file is read again only when its size or time changed. A rewrite
// to the same bytes (a Connect writes it again) is the same tag, and
// restarts nothing.
func codexCatalogTag(path string) string {
	st, err := os.Stat(path)
	if err != nil {
		return ""
	}
	c := &codexCatalogSeen
	c.Lock()
	defer c.Unlock()
	if c.path == path && c.size == st.Size() && c.mod.Equal(st.ModTime()) {
		return c.tag
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	sum := sha256.Sum256(b)
	c.path, c.size, c.mod, c.tag = path, st.Size(), st.ModTime(), hex.EncodeToString(sum[:12])
	return c.tag
}

// codexCatalogSeen is the last catalog codexCatalogTag read.
var codexCatalogSeen struct {
	sync.Mutex
	path string
	size int64
	mod  time.Time
	tag  string
}
