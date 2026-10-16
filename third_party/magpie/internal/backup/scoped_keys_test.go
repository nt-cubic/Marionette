package backup

import (
	"encoding/json"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/settings"
)

func TestRestoreLibraryKeysScope(t *testing.T) {
	for _, policy := range []string{"keyless", "full", "settings", "providers", "library"} {
		for _, selected := range []bool{false, true} {
			name := policy + "/excluded"
			if selected {
				name = policy + "/selected"
			}
			t.Run(name, func(t *testing.T) {
				home(t)
				appdir.UseExecutable("")
				cur := settings.Load()
				cur.GitHubToken = "fixture-local-github"
				if err := settings.Save(cur); err != nil {
					t.Fatal(err)
				}
				if _, err := library.SaveServer("", library.Server{Name: "github", Transport: "stdio", Command: "fixture-mcp",
					Env: map[string]string{"GITHUB_TOKEN": "fixture-local-mcp"}, Agents: []string{}}); err != nil {
					t.Fatal(err)
				}
				if _, err := library.SaveServer("", library.Server{Name: "headers", Transport: "http", URL: "https://mcp.example.com",
					Headers: map[string]string{"Authorization": "fixture-local-header"}, Agents: []string{}}); err != nil {
					t.Fatal(err)
				}
				b := Bundle{Version: BundleVersion, Keys: policy == "full", Library: &library.Bundle{MCP: []*library.Server{{Name: "github", Transport: "stdio", Command: "changed-mcp",
					Env: map[string]string{"GITHUB_TOKEN": ""}, Agents: []string{}}, {Name: "headers", Transport: "http", URL: "https://mcp.example.com",
					Headers: map[string]string{"Authorization": ""}, Agents: []string{}}}}}
				if policy != "keyless" && policy != "full" {
					// Read a serialized scoped bundle, as an imported backup would.
					if err := json.Unmarshal([]byte(`{"`+policy+`Keys":true}`), &b); err != nil {
						t.Fatal(err)
					}
				}
				if _, err := Restore(b, Parts{Library: selected}); err != nil {
					t.Fatal(err)
				}
				got, err := library.Collect()
				if err != nil {
					t.Fatal(err)
				}
				wantToken, wantHeader, wantCommand := "fixture-local-mcp", "fixture-local-header", "fixture-mcp"
				if selected {
					wantCommand = "changed-mcp"
				}
				if got.MCP[0].Command != wantCommand || got.MCP[0].Env["GITHUB_TOKEN"] != wantToken || got.MCP[1].Headers["Authorization"] != wantHeader {
					t.Errorf("library restoration ignored its selected credential scope: command=%q, token=%q, header=%q", got.MCP[0].Command, got.MCP[0].Env["GITHUB_TOKEN"], got.MCP[1].Headers["Authorization"])
				}
				if settings.Load().GitHubToken != cur.GitHubToken {
					t.Error("restoring the library changed an unselected settings credential")
				}
			})
		}
	}
}

func TestRestoreLibraryCredentialUpdates(t *testing.T) {
	for _, scoped := range []bool{false, true} {
		for _, action := range []string{"replace", "remove-keys", "remove-server"} {
			policy := "full"
			if scoped {
				policy = "library"
			}
			t.Run(policy+"/"+action, func(t *testing.T) {
				home(t)
				appdir.UseExecutable("")
				server := library.Server{Name: "github", Transport: "stdio", Command: "fixture-mcp", Agents: []string{},
					Env: map[string]string{"GITHUB_TOKEN": "fixture-local", "MODE": "local"}}
				if _, err := library.SaveServer("", server); err != nil {
					t.Fatal(err)
				}
				headers := library.Server{Name: "headers", Transport: "http", URL: "https://mcp.example.com", Agents: []string{},
					Headers: map[string]string{"Authorization": "fixture-local-header", "X-Mode": "local"}}
				if _, err := library.SaveServer("", headers); err != nil {
					t.Fatal(err)
				}
				server.Env = map[string]string{"GITHUB_TOKEN": "fixture-new", "MODE": ""}
				headers.Headers = map[string]string{"Authorization": "fixture-new-header", "X-Mode": ""}
				b := Bundle{Version: BundleVersion, Keys: !scoped, Library: &library.Bundle{MCP: []*library.Server{&server, &headers}}}
				if scoped {
					b.LibraryKeys = Flag(true)
				}
				switch action {
				case "remove-keys":
					delete(server.Env, "GITHUB_TOKEN")
					delete(headers.Headers, "Authorization")
				case "remove-server":
					b.Library.MCP = nil
				}
				if _, err := Restore(b, Parts{Library: true}); err != nil {
					t.Fatal(err)
				}
				got, err := library.Collect()
				if err != nil {
					t.Fatal(err)
				}
				if action == "remove-server" {
					if len(got.MCP) != 0 {
						t.Fatal("restoring credentials resurrected a removed server")
					}
					return
				}
				if len(got.MCP) != 2 || got.MCP[0].Env["MODE"] != "" || got.MCP[1].Headers["X-Mode"] != "" {
					t.Fatal("a non-secret blank was replaced with a local value")
				}
				if action == "replace" {
					if got.MCP[0].Env["GITHUB_TOKEN"] != "fixture-new" || got.MCP[1].Headers["Authorization"] != "fixture-new-header" {
						t.Fatal("a nonempty incoming credential did not replace the local value")
					}
				} else {
					if _, ok := got.MCP[0].Env["GITHUB_TOKEN"]; ok {
						t.Error("a removed environment key came back")
					}
					if _, ok := got.MCP[1].Headers["Authorization"]; ok {
						t.Error("a removed header came back")
					}
				}
			})
		}
	}
}
