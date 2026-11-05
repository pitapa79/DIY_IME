param([string]$Cargo = 'cargo')
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$stage = Join-Path $root ('target/core-boundary/' + [guid]::NewGuid().ToString('N'))
$runtime = Join-Path $stage 'crates/runtime'
[IO.Directory]::CreateDirectory((Split-Path $runtime)) | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'crates/runtime') -Destination $runtime -Recurse
[IO.Directory]::CreateDirectory((Join-Path $stage 'tests/contracts')) | Out-Null
# The Core unit tests use canonical data fixtures, not Host/Node implementation code.
Get-ChildItem -LiteralPath (Join-Path $root 'tests') -Filter '*.json' -File | Copy-Item -Destination (Join-Path $stage 'tests')
Get-ChildItem -LiteralPath (Join-Path $root 'tests/contracts') -Filter '*.json' -File | Copy-Item -Destination (Join-Path $stage 'tests/contracts')
$manifest = Join-Path $runtime 'Cargo.toml'
[IO.File]::AppendAllText($manifest, "`n[workspace]`n", [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath (Join-Path $root 'Cargo.lock') -Destination (Join-Path $runtime 'Cargo.lock')
if (Test-Path -LiteralPath (Join-Path $stage 'src')) { throw 'Host sources unexpectedly present' }
& $Cargo generate-lockfile --manifest-path $manifest --offline
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve isolated Core lockfile' }
# Isolation must not silently select a different third-party dependency set.
$sourcePackages = @{}; $identity = $null
foreach ($line in [IO.File]::ReadAllLines((Join-Path $root 'Cargo.lock'))) {
 if ($line -match '^name = "(.+)"') { $identity = $Matches[1] }
 if ($line -match '^version = "(.+)"' -and $null -ne $identity) { $sourcePackages[$identity] = $Matches[1] }
}
$identity = $null
foreach ($line in [IO.File]::ReadAllLines((Join-Path $runtime 'Cargo.lock'))) {
 if ($line -match '^name = "(.+)"') { $identity = $Matches[1] }
 if ($line -match '^version = "(.+)"' -and $null -ne $identity) {
  if ($identity -eq 'diy-ime2-core' -or $sourcePackages[$identity] -ne $Matches[1]) { throw "Isolated dependency differs: $identity" }
 }
}
& $Cargo test --manifest-path $manifest --locked --offline --lib
if ($LASTEXITCODE -ne 0) { throw 'Isolated Core tests failed' }
Write-Output 'Core builds and tests without Host or production Node source'