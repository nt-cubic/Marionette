package plugin

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// TestPackageJSONTailIsCut: plugins#405's package.json, byte for byte (a
// bun remove on Windows wrote a shorter one over the old without
// truncating it), parses again after a bun command, keeps every
// dependency, and the file as it was stays beside it. A file whose start
// isn't an object, and one that parses, are left as they are.
func TestPackageJSONTailIsCut(t *testing.T) {
	const kept = "{\n  \"name\": \"magpie-plugins\",\n  \"private\": true,\n  \"dependencies\": {\n    \"@magpie-community/opencode-qoder-auth\": \"0.2.33\",\n    \"@magpie-community/opencode-workbuddy-auth\": \"0.1.29\"\n  }\n}\n"
	const reported = kept + "@magpie-community/opencode-zcode-auth\": \"^0.1.27\"\n  }\n}\n"
	for _, tc := range []struct{ name, body, want string }{
		{"a tail after the object", reported, kept},
		{"whole", kept, kept},
		{"no object to keep", "@magpie\": 1\n}\n", "@magpie\": 1\n}\n"},
		{"cut short", "{\n  \"name\": \"magpie-plugins\",\n", "{\n  \"name\": \"magpie-plugins\",\n"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			storeSandbox(t)
			if err := os.MkdirAll(Dir(), 0o755); err != nil {
				t.Fatal(err)
			}
			pj := filepath.Join(Dir(), "package.json")
			if err := os.WriteFile(pj, []byte(tc.body), 0o644); err != nil {
				t.Fatal(err)
			}
			mendPackageJSON()
			got, err := os.ReadFile(pj)
			if err != nil {
				t.Fatal(err)
			}
			if string(got) != tc.want {
				t.Fatalf("package.json is\n%s\nwant\n%s", got, tc.want)
			}
			broken, err := os.ReadFile(pj + ".broken")
			if tc.body == tc.want {
				if err == nil {
					t.Fatalf("a file left alone was copied to package.json.broken")
				}
				return
			}
			if string(broken) != tc.body {
				t.Fatalf("package.json.broken is\n%s\nwant the file as it was", broken)
			}
			var pkg struct{ Dependencies map[string]string }
			if err := json.Unmarshal(got, &pkg); err != nil || len(pkg.Dependencies) != 2 {
				t.Fatalf("mended file gave %v, %v", pkg.Dependencies, err)
			}
		})
	}
}
