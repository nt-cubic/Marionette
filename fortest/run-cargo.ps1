$ErrorActionPreference = 'Continue'
$out = 'D:\Project\Marionette\fortest\cargo-out.txt'
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
Set-Location 'D:\Project\Marionette\src-tauri'
$lines = @()
$lines += "=== cargo verify started $(Get-Date -Format o) ==="
$lines += "cwd=$(Get-Location)"
$lines += "cargo=$(Get-Command cargo -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source)"

function Run-Step($name, $args) {
  $lines += ""
  $lines += "=== $name ==="
  try {
    $output = & cargo @args 2>&1 | ForEach-Object { $_.ToString() }
    $ec = $LASTEXITCODE
    $lines += $output
    $lines += ""
    $lines += "${name}_exit_code=$ec"
    return $ec
  } catch {
    $lines += "EXCEPTION: $_"
    $lines += "${name}_exit_code=-1"
    return -1
  }
}

$ec1 = Run-Step 'elicitation_test' @('test','--bin','marionette','elicitation','--','--nocapture')
$ec2 = Run-Step 'exit_plan_mode_test' @('test','--bin','marionette','exit_plan_mode','--','--nocapture')
$ec3 = Run-Step 'cargo_check' @('check','--bin','marionette')
$lines += "=== cargo verify finished $(Get-Date -Format o) ==="
$lines += "SUMMARY elicitation=$ec1 exit_plan_mode=$ec2 check=$ec3"
$lines | Set-Content -Path $out -Encoding utf8
Write-Host "Wrote $out"
