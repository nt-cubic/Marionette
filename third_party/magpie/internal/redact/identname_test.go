package redact

import (
	"encoding/json"
	"strings"
	"testing"
)

// A user name a rule found in a home folder is masked again wherever it
// stands as the user's name, but not where code has the same word as a
// name of its own: a parameter, a variable, a JSON key, a part of a
// snake_case or camelCase name (Tomyu_2034 on X: "masking 很容易把参数变量名
// 当成 user name 给 mask 掉"). A user name such as developer is a word code
// has too. The texts are what an agent sends: a file it read, a diff, a
// tool call's arguments.
func TestUserNameInCodeIsNoUserName(t *testing.T) {
	Forget()
	defer Forget()
	o := Options{Personal: true, Kinds: map[string]bool{"home": true, "userhost": true}}
	for _, name := range []string{"developer", "rinakato"} {
		f := standInOf(name)
		// the system prompt's working folder: the name is known from here on
		if out, _ := Mask("Working directory: /Users/"+name+"/proj", o); !strings.Contains(out, "/Users/"+f+"/proj") {
			t.Fatalf("%s: home folder sent as %q", name, out)
		}
		r := strings.NewReplacer("NAME", name)
		for _, s := range []string{
			// a file read
			r.Replace("def run(NAME, NAME_dir: str):\n    NAME_mode = True\n    self.NAME = NAME\n    return NAME\n"),
			r.Replace("const NAME = loadUser();\nlet NAMEConfig = {};\nfunction greet(NAME) { return NAME(1) }\n"),
			r.Replace(`{"NAME": true, "NAME_id": 3, "NAME-config": "x"}`),
			r.Replace("import NAME from './NAME.js'\nfunc New(NAME string, opts Options) {}\nNAME := cfg.NAME\n"),
			// a diff
			r.Replace("@@ -1,3 +1,3 @@\n-    NAME_count := 0\n+    NAMECount := 0\n     var NAME int\n     if NAME == 0 {\n"),
		} {
			if out, _ := Mask(s, o); out != s {
				t.Errorf("%s: code sent as\n%s\nwant\n%s", name, out, s)
			}
		}
		// a tool call's arguments
		body := `{"messages":[{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"edit","arguments":` +
			jsonString(r.Replace(`{"file_path":"src/app.py","old_string":"def run(NAME):","new_string":"def run(NAME, NAME_dir):"}`)) + `}}]}]}`
		if out, _ := MaskJSON([]byte(body), o); string(out) != body {
			t.Errorf("%s: arguments sent as\n%s", name, out)
		}

		// and where it is the user's name, it goes as the made-up name
		for _, c := range [][2]string{
			{"cat /Users/NAME/.zshrc", "cat /Users/F/.zshrc"},
			{"git log --author=NAME", "git log --author=F"},
			{"drwxr-xr-x  5 NAME  staff  160 Nov  2 10:00 proj", "drwxr-xr-x  5 F  staff  160 Nov  2 10:00 proj"},
			{"uid=501(NAME) gid=20(staff)", "uid=501(F) gid=20(staff)"},
			{"Thanks, NAME! The build is green.", "Thanks, F! The build is green."},
			{"I'm NAME.", "I'm F."},
			{`os.chown(path, "NAME")`, `os.chown(path, "F")`},
			{`{"owner": "NAME"}`, `{"owner": "F"}`},
			{"chown NAME:staff app.log", "chown F:staff app.log"},
		} {
			s, want := r.Replace(c[0]), strings.NewReplacer("F", f).Replace(r.Replace(c[1]))
			if out, _ := Mask(s, o); out != want {
				t.Errorf("%s: %q sent as %q, want %q", name, s, out, want)
			}
		}
		args := `{"messages":[{"role":"assistant","tool_calls":[{"id":"call_2","type":"function","function":{"name":"bash","arguments":` +
			jsonString(r.Replace(`{"command":"ls -la ~NAME && echo NAME"}`)) + `}}]}]}`
		if out, _ := MaskJSON([]byte(args), o); strings.Contains(string(out), name) || !strings.Contains(string(out), "~"+f+" && echo "+f) {
			t.Errorf("%s: arguments sent as\n%s", name, out)
		}
	}
}

// A "User:" line's value in code is a variable's or a type's name, not a
// login: user = current_user, user: UserInfo, a Go struct's User field.
// Written out (quoted, or a login of words of its own), it is masked.
func TestUserLineInCodeIsNoUserName(t *testing.T) {
	Forget()
	defer Forget()
	o := Options{Personal: true, Kinds: map[string]bool{"userhost": true}}
	for _, s := range []string{
		"user = current_user",
		"    user: UserInfo",
		"login = loginName",
		"username = target_name",
		"user = user_obj",
		"type Session struct {\n\tUser   UserProfile\n}",
		"export interface Props {\n  user: CurrentUser\n}",
	} {
		if out, n := Mask(s, o); n != 0 {
			t.Errorf("%q sent as %q", s, out)
		}
	}
	for _, c := range [][2]string{
		{"User: deploy_bot", "deploy_bot"},
		{"USER=rinak", "rinak"},
		{`user: "current_user"`, "current_user"},
		{"Host box\n  User rinak", "rinak"},
		{"user = john_doe", "john_doe"},
	} {
		if out, n := Mask(c[0], o); n != 1 || strings.Contains(out, c[1]) {
			t.Errorf("%q sent as %q, want %s masked", c[0], out, c[1])
		}
	}
}

// jsonString is s as a JSON string.
func jsonString(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}
