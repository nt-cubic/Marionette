package redact

// ModelKinds is modelKinds, for the evaluation in package redact_test,
// which reads the real model through internal/pii (eval_test.go).
var ModelKinds = modelKinds

// Forget lets go of every value masked so far, so that what the
// evaluation scores is what its texts make masked, not what the tests
// before it masked and left known.
func Forget() {
	mu.Lock()
	values = map[string]string{}
	shapes = map[string]string{}
	reindexKnown()
	mu.Unlock()
	resetSent()
}
