package cli

import (
	"go/ast"
	"go/parser"
	"go/token"
	"path/filepath"
	"strings"
	"testing"
)

// TestGatewayStartsGetUserPath: every way magpie starts with a gateway
// gives what it starts the node a version manager put on PATH only in the
// shell's profile (#2685, #3803). magpie tui runs in a terminal, with its
// PATH already.
func TestGatewayStartsGetUserPath(t *testing.T) {
	want := map[string]bool{"runGUI": false, "runPanel": false, "webCmd": false, "serveCmd": false}
	files, _ := filepath.Glob("*.go")
	fset := token.NewFileSet()
	for _, f := range files {
		if strings.HasSuffix(f, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(fset, f, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		for _, d := range file.Decls {
			fn, ok := d.(*ast.FuncDecl)
			if !ok || fn.Body == nil {
				continue
			}
			if _, named := want[fn.Name.Name]; !named {
				continue
			}
			ast.Inspect(fn.Body, func(n ast.Node) bool {
				if sel, ok := n.(*ast.SelectorExpr); ok && sel.Sel.Name == "UserPath" {
					if id, ok := sel.X.(*ast.Ident); ok && id.Name == "proc" {
						want[fn.Name.Name] = true
					}
				}
				return true
			})
		}
	}
	for name, calls := range want {
		if !calls {
			t.Errorf("%s doesn't call proc.UserPath", name)
		}
	}
}
