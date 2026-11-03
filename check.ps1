# Aurora — одна команда на все проверки (CLAUDE.md contract).
# Использование:  ./check.ps1          (Rust: fmt + clippy + test, затем фронтенд)
#                 ./check.ps1 -Rust    (только Rust)
#                 ./check.ps1 -Frontend (только фронтенд)
# Exit code 0 = всё зелёное; иначе печатает, какой шаг упал.
param(
  [switch]$Rust,
  [switch]$Frontend
)
if (-not $Rust -and -not $Frontend) { $Rust = $true; $Frontend = $true }

$root = "C:\Users\madei\OneDrive\Desktop\aurora"
Set-Location $root
$fail = $null

function Step($name, $script) {
  if ($fail) { return }
  Write-Host "`n==> $name" -ForegroundColor Cyan
  & $script
  if ($LASTEXITCODE -ne 0 -and -not $fail) { $fail = $name }
}

if ($Rust) {
  Step "cargo fmt --all" { cargo fmt --all }
  Step "cargo clippy (workspace, -D warnings)" { cargo clippy --workspace --all-targets -- -D warnings }
  Step "cargo test (workspace)" { cargo test --workspace }
}

if ($Frontend -and -not $fail) {
  Step "tsc --noEmit" { Set-Location "$root\frontend"; npx tsc --noEmit }
  Step "vite build" { npm run build }
}

Set-Location $root
if ($fail) {
  Write-Host "`nFAILED: $fail" -ForegroundColor Red
  exit 1
}
Write-Host "`nOK: all checks passed" -ForegroundColor Green
exit 0
