//go:build comlauncher

package update

import _ "embed"

// The launcher magpie.com is made of (com.go), built for windows/amd64 by
// `make release-windows` before the app; not kept in git.
//
//go:embed launcher-amd64.com
var launcheramd64 []byte

func init() { comLauncher = launcheramd64 }
