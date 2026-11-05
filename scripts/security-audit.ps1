# Dependency-only audit. Do not pass input text, Node settings, or credentials.
[CmdletBinding()]
param(
    [string]$OutputDirectory,
    [string]$CargoAuditExecutable
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root 'target/security-audit' }
[IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'summary.json'),'{"status":"incomplete"}')
$lockfiles = @((Join-Path $root 'Cargo.lock'), (Join-Path $root 'editor/package-lock.json'))
$before = @($lockfiles | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash })
$version = '0.22.2'
# Official rustsec/rustsec release asset digest, verified 2026-10-31.
$digest = '0a7316540862c13d954f648917ceacca593747baed6eec180fafa590be2710ab'
if (-not $CargoAuditExecutable) {
    if (-not $IsWindows) { throw 'Pinned audit binary is Windows x64 only; supply an approved cargo-audit executable on other platforms.' }
    $archive = Join-Path $OutputDirectory 'cargo-audit.zip'
    $tools = Join-Path $OutputDirectory 'tools'
    Invoke-WebRequest -Uri "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v$version/cargo-audit-x86_64-pc-windows-msvc-v$version.zip" -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $digest) {
        throw 'Official cargo-audit archive digest mismatch'
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $tools -Force
    $CargoAuditExecutable = Join-Path $tools "cargo-audit-x86_64-pc-windows-msvc-v$version/cargo-audit.exe"
}
$auditVersion = & $CargoAuditExecutable --version
if ($LASTEXITCODE -ne 0 -or "$auditVersion" -ne "cargo-audit $version") { throw 'Unexpected cargo-audit executable version' }
Push-Location $root
try {
    $db = Join-Path $OutputDirectory 'rustsec-db'
    $databaseUrl = 'https://github.com/RustSec/advisory-db.git'
    if (Test-Path -LiteralPath $db) {
        $origin = & git -C $db remote get-url origin
        if ($LASTEXITCODE -ne 0 -or "$origin" -ne $databaseUrl) { throw 'Unexpected audit database origin' }
        & git -c credential.helper= -c credential.interactive=false -c http.lowSpeedLimit=1 -c http.lowSpeedTime=30 -C $db fetch --depth 1 origin HEAD
        if ($LASTEXITCODE -ne 0) { throw 'RustSec database update failed' }
        & git -C $db checkout --detach FETCH_HEAD
    } else {
        & git -c credential.helper= -c credential.interactive=false -c http.lowSpeedLimit=1 -c http.lowSpeedTime=30 clone --depth 1 $databaseUrl $db
    }
    if ($LASTEXITCODE -ne 0) { throw 'RustSec database unavailable' }
    $databaseCommit = & git -C $db rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'RustSec database revision unavailable' }
    $rustLines = & $CargoAuditExecutable audit --file Cargo.lock --db $db --no-fetch --format json --deny warnings 2> (Join-Path $OutputDirectory 'rust-stderr.log')
    $rustExit = $LASTEXITCODE
    $rustJson = $rustLines -join [Environment]::NewLine
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'rust.json'),$rustJson)
    $rust = $rustJson | ConvertFrom-Json
    if ($null -eq $rust.vulnerabilities -or $null -eq $rust.database) { throw 'Incomplete Rust audit result' }
    Push-Location (Join-Path $root 'editor')
    try {
        $npmCommand = if ($IsWindows) { 'npm.cmd' } else { 'npm' }
        $npmLines = & $npmCommand audit --package-lock-only --ignore-scripts --audit-level=high --json 2> (Join-Path $OutputDirectory 'npm-stderr.log')
        $npmExit = $LASTEXITCODE
    } finally { Pop-Location }
    $npmJson = $npmLines -join [Environment]::NewLine
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'npm.json'),$npmJson)
    $npm = $npmJson | ConvertFrom-Json
    if ($null -eq $npm.metadata.vulnerabilities) { throw 'Incomplete npm audit result' }
    $after = @($lockfiles | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash })
    if ($before[0] -ne $after[0] -or $before[1] -ne $after[1]) { throw 'Audit unexpectedly changed a lockfile' }
    $summary = [ordered]@{
        status = if ($rustExit -eq 0 -and $npmExit -eq 0) { 'passed' } else { 'failed' }
        auditedAtUtc = [DateTime]::UtcNow.ToString('o')
        cargoAudit = "$auditVersion"
        rustDatabase = $rust.database
        rustDatabaseCommit = "$databaseCommit"
        rustExit = $rustExit
        rustVulnerabilities = $rust.vulnerabilities.count
        npmExit = $npmExit
        npmVulnerabilities = $npm.metadata.vulnerabilities
        cargoLockSha256 = $after[0]
        npmLockSha256 = $after[1]
    }
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'summary.json') -Encoding utf8
    $summary | ConvertTo-Json -Depth 8
    if ($rustExit -ne 0 -or $npmExit -ne 0) { exit 1 }
} finally { Pop-Location }
