package redact

import (
	"strings"
	"testing"
)

// A package's name, in every shape code and manifests write one, goes as
// written by the rules, with the default categories and with all of them
// (dniray on Discord, v0.1.1917: "脱敏会把包名给替换了").
func TestPackageNamesGoAsWritten(t *testing.T) {
	for _, s := range []string{
		// npm, scoped and pinned
		"npm install @babel/core @types/node @vue/cli-service --save-dev",
		"pnpm add @tanstack/react-query@5.59.0 @scope/name@1.2.3 lodash@4.17.21",
		"import { and, eq } from ' drizzle-orm '",
		"import { and, eq } from 'drizzle-orm'",
		"const jwt = require( ' jsonwebtoken ' );",
		`"node_modules/@babel/code-frame": {"version": "7.23.5"}`,
		// Java and Android
		"package com.example.app;\nimport org.springframework.boot.SpringApplication;",
		"包名是 com.dniray.tool，帮我改成 com.zhangsan.demo",
		"implementation 'com.squareup.retrofit2:retrofit:2.9.0'",
		// Go, Python, purl, Debian, Cargo
		"go get github.com/spf13/cobra@v1.8.0 golang.org/x/net",
		"from sklearn.model_selection import train_test_split",
		"pip install requests==2.31.0 scikit-learn pandas>=2.0",
		"purl: pkg:npm/%40angular/animation@12.3.1 pkg:pypi/django@1.11.1",
		"sudo apt-get install -y libssl-dev libffi-dev python3-dev",
		"cargo add serde_json tokio; use jsonwebtoken::encode;",
		// a dependency with a secret's word in its name, at a prerelease
		`{"dependencies": {"js-tokens": "^4.0.0-beta.1", "tokenizers": "0.15.0-rc1", "next-auth": "5.0.0-beta.25"}}`,
		"[dependencies]\njsonwebtoken = \"9.2.0-beta1\"\npassword-hash = \"0.5.0-pre.1\"",
		"secretVersion = '2.1.0-alpha03'\ntokenizer = \"2.4.0-beta02\"",
		"tokenizers==0.15.0rc1\npassword_strength==0.0.3.post2\ncredentials-manager==0.2.0b1",
	} {
		for _, o := range []Options{{Secrets: true, Personal: true}, allKinds()} {
			if out, n := Mask(s, o); n != 0 {
				t.Errorf("all=%v: %q masked as %q", o.Kinds != nil, s, out)
			}
		}
	}
}

// pnpm-lock.yaml lists each package pinned at its version on a line of
// its own, a shell prompt's shape: with user@host on, the package and
// its version went as a user and a host.
func TestPackagePinIsNoLogin(t *testing.T) {
	for _, s := range []string{
		"packages:\n  js-tokens@4.0.0:\n    resolution: {}",
		"  react-dom@18.2.0-rc.0:\n",
		"ssh deploy@1.2.3-beta ~/x",
	} {
		if out, n := Mask(s, allKinds()); n != 0 {
			t.Errorf("%q masked as %q", s, out)
		}
	}
}

// What the local model took for a secret in a package's name: the
// module an import loads (dniray's literal bytes first), a requirement's
// name, a lock file's integrity hash.
func TestModelPackageNamesNotMasked(t *testing.T) {
	for _, c := range []struct{ text, value string }{
		{"import { and, eq } from ' drizzle-orm '", "drizzle-orm"},
		{"import { and, eq } from 'drizzle-orm';", "drizzle-orm"},
		{"} from \"drizzle-orm/pg-core\";", "drizzle-orm/pg-core"},
		{"export * from '@acme/secret-store';", "@acme/secret-store"},
		{"import 'reflect-metadata'", "reflect-metadata"},
		{"const jwt = require( ' jsonwebtoken ' );", "jsonwebtoken"},
		{"const m = await import('node:crypto')", "node:crypto"},
		{"from secretstorage import collection", "secretstorage"},
		{"import jsonwebtoken as jwt", "jsonwebtoken"},
		{"use jsonwebtoken::{encode, Header};", "jsonwebtoken"},
		{"django-rest-passwordreset==1.4.0\n", "django-rest-passwordreset"},
		{"tokenizers>=0.15\n", "tokenizers"},
		{"resolution: {integrity: sha512-RdJUflcE3cUzKiMqQgsCu06FPu9UdIJO0beYbPhHN4k6apgJtifcoCtT9bcxOpYBtpD2kCM6Sbzg4CausW/PKQ==}",
			"sha512-RdJUflcE3cUzKiMqQgsCu06FPu9UdIJO0beYbPhHN4k6apgJtifcoCtT9bcxOpYBtpD2kCM6Sbzg4CausW/PKQ==}"},
	} {
		f := &fakeModel{values: map[string]string{c.value: "secret"}}
		if out, n := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); n != 0 {
			t.Errorf("%q: masked %q", c.value, out)
		}
	}
}

// Real values beside the shapes above are masked still.
func TestPackageNameChecksKeepRealValues(t *testing.T) {
	for _, c := range []struct{ text, value string }{
		{"Contact zhang.wei@acme-corp.cn about it", "zhang.wei@acme-corp.cn"},
		{"ssh deploy@build01.corp.acme-internal.net ~/run.sh", "build01.corp.acme-internal.net"},
		{"password=hunter2pass9", "hunter2pass9"},
		{"API_KEY=AbC123dEf456GhI789", "AbC123dEf456GhI789"},
		{"secret_version = 'Zq8vR2mT6wLp'", "Zq8vR2mT6wLp"},
		{"alice@10.0.0.5:~$ ls", "alice"},
	} {
		if out, _ := Mask(c.text, allKinds()); strings.Contains(out, c.value) {
			t.Errorf("%q left in %q", c.value, out)
		}
	}
	// the model's secret in prose, after a password, before an address
	for _, c := range []struct{ text, value string }{
		{"I copied the token from 'Zq8vR2mT6wLp' yesterday", "Zq8vR2mT6wLp"},
		{"copy it from Zq8vR2mT6wLp now", "Zq8vR2mT6wLp"},
		{"the vault phrase is Zq8vR2mT6wLp", "Zq8vR2mT6wLp"},
		{"mysql -p s3cretPass@10.0.0.5", "s3cretPass"},
		{"pw Zq8vR2mT6wLp==", "Zq8vR2mT6wLp"},
	} {
		f := &fakeModel{values: map[string]string{c.value: "secret"}}
		if out, _ := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); strings.Contains(out, c.value) {
			t.Errorf("%q left in %q", c.value, out)
		}
	}
}
