@echo off
rem Local equivalent of .github/workflows/ci.yml — the full gate in one go.
rem Usage: tools\ci.bat   (run from anywhere; paths resolve off this file)
setlocal
set "ROOT=%~dp0.."
cd /d "%ROOT%" || exit /b 1

echo ── 1/4 typecheck ──────────────────────────────────────────────
call npm run typecheck || (echo [ci] typecheck FAILED & exit /b 1)

echo ── 2/4 verify scripts ─────────────────────────────────────────
call npm run verify || (echo [ci] verify FAILED & exit /b 1)

echo ── 3/4 cargo test ─────────────────────────────────────────────
pushd src-tauri
call cargo test || (echo [ci] cargo test FAILED & popd & exit /b 1)
echo ── 4/4 cargo debug build ──────────────────────────────────────
call cargo build || (echo [ci] cargo build FAILED & popd & exit /b 1)
popd

echo [ci] all green
exit /b 0
