Vendored from https://github.com/yetone/magpie (MIT) at tag v0.1.2597 on 2026-11-01.
Imported by tools\import-magpie.ps1 â€?default Tag is GitHub latest stable; pass -Tag or MAGPIE_TAG to pin.
Subset only: build inputs for the nogui CLI. Upstream site/, build/, .github/, docs/ are intentionally absent.
Marionette patch: main_seed.go (build tag marionette_seed) is the slim gateway-only main; import re-applies it. See tools\magpie-seed\main_seed.go.
Marionette patch: marionette_peel.go peels Codex [effort] (and Claude [1m]) in GroupFor so share aliases match. See tools\magpie-seed\marionette_peel.go.
License: LICENSE (MIT) in this folder.
