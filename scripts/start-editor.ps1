param([switch]$Check)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$editorRoot = Join-Path $repoRoot 'editor'

function Invoke-Step {
    param([string]$Executable, [string[]]$Arguments)
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Command failed ($LASTEXITCODE): $Executable $Arguments" }
}

# Prefer the user's Node when compatible, then the app's bundled runtime.
# PATH changes below apply only to this script and its child processes.
$nodeCommand = Get-Command node.exe -ErrorAction SilentlyContinue
$nodeCandidates = @()
if ($nodeCommand) { $nodeCandidates += $nodeCommand.Source }
$nodeCandidates += Join-Path $env:USERPROFILE '.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe'
$nodeExe = $null
foreach ($candidate in $nodeCandidates) {
    if (-not (Test-Path -LiteralPath $candidate)) { continue }
    $version = [Version]((& $candidate --version).Trim().TrimStart('v'))
    if (($version.Major -eq 22 -and $version -ge [Version]'22.22.2') -or
        ($version.Major -eq 24 -and $version -ge [Version]'24.15.0') -or $version.Major -ge 26) {
        $nodeExe = $candidate
        break
    }
}
if (-not $nodeExe) { throw 'Install Node.js 22.22.2+ (22 LTS), 24.15+ (24 LTS), or 26+ before starting the editor.' }
$env:PATH = (Split-Path -Parent $nodeExe) + ';' + $env:PATH

$npmCommand = Get-Command npm.cmd -ErrorAction SilentlyContinue
$npmCandidates = @(
    (Join-Path (Split-Path -Parent $nodeExe) 'node_modules\npm\bin\npm-cli.js'),
    (Join-Path $env:ProgramFiles 'nodejs\node_modules\npm\bin\npm-cli.js')
)
if ($npmCommand) { $npmCandidates += Join-Path (Split-Path -Parent $npmCommand.Source) 'node_modules\npm\bin\npm-cli.js' }
$npmCli = $npmCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $npmCli) { throw 'npm was not found. Install Node.js with npm.' }

Push-Location $repoRoot
try {
    $cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
    $taskRustDir = Join-Path $env:TEMP 'diy-ime2-rust'
    $rustup = Join-Path $taskRustDir 'rustup.exe'
    $core = Join-Path $repoRoot 'target\debug\diy-ime2.exe'
    $cargoExe = $null
    $cargoPrefix = @()
    if ($cargoCommand) { $cargoExe = $cargoCommand.Source }
    elseif (Test-Path -LiteralPath $rustup) {
        $env:RUSTUP_HOME = Join-Path $taskRustDir 'rustup'
        $env:CARGO_HOME = Join-Path $taskRustDir 'cargo'
        $cargoExe = $rustup
        $cargoPrefix = @('run', 'stable-x86_64-pc-windows-gnu', 'cargo')
    }
    if ($cargoExe) {
        Invoke-Step $cargoExe ($cargoPrefix + @('build', '--locked', '--bin', 'diy-ime2'))
        if ($Check) {
            Invoke-Step $cargoExe ($cargoPrefix + @('build', '--locked', '--example', 'execution_broker_gate', '--example', 'execution_transport_fixtures'))
            Invoke-Step $cargoExe ($cargoPrefix + @('test', '--locked'))
        }
    } else {
        if (-not (Test-Path -LiteralPath $core)) { throw 'Build the Rust core first: cargo build --locked --bin diy-ime2' }
        $sources = @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'src') -Filter '*.rs' -Recurse) + @(Get-Item -LiteralPath (Join-Path $repoRoot 'Cargo.toml'))
        $newest = ($sources | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1).LastWriteTimeUtc
        if ((Get-Item -LiteralPath $core).LastWriteTimeUtc -lt $newest) { throw 'Rust sources changed. Install Rust and rebuild: cargo build --locked --bin diy-ime2' }
        Write-Host 'Using the existing Rust executable (Cargo is not installed).'
    }

    Set-Location -LiteralPath $editorRoot
    $installedLock = Join-Path $editorRoot 'node_modules\.package-lock.json'
    $lock = Join-Path $editorRoot 'package-lock.json'
    if (-not (Test-Path -LiteralPath $installedLock) -or
        (Get-Item -LiteralPath $lock).LastWriteTimeUtc -gt (Get-Item -LiteralPath $installedLock).LastWriteTimeUtc) {
        Invoke-Step $nodeExe @($npmCli, 'ci', '--no-audit', '--no-fund')
    }
    if ($Check) {
        Invoke-Step $nodeExe @($npmCli, 'test')
        Invoke-Step $nodeExe @($npmCli, 'run', 'build')
    } else {
        Write-Host 'Editor: http://127.0.0.1:5173/  (Ctrl+C to stop)'
        Invoke-Step $nodeExe @('node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '5173')
    }
} finally { Pop-Location }
