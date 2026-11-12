Vendored from https://github.com/yetone/magpie (MIT) at tag v0.1.3899 on 2026-11-12.
Imported by tools\import-magpie.ps1 â€?default Tag is GitHub latest stable; pass -Tag or MAGPIE_TAG to pin.
Subset only: build inputs for the nogui CLI, plus upstream third_party/ when the tag ships local go.mod replacements. Upstream site/, build/, .github/, docs/ are intentionally absent.
Marionette patch: main_seed.go (build tag marionette_seed) is the slim gateway-only main; import re-applies it. See tools\magpie-seed\main_seed.go.
Marionette patch: marionette_peel.go peels Codex [effort] (and Claude [1m]) in GroupFor so share aliases match. See tools\magpie-seed\marionette_peel.go.
Marionette patch: marionette_body_cap.go counts ollama.com's 'failed to read request body' 400 as a byte-cap refusal, so a Codex turn over the cap drops its oldest images instead of stalling. See tools\magpie-seed\marionette_body_cap.go.
License: LICENSE (MIT) in this folder.
