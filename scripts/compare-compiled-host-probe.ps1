param(
  [Parameter(Mandatory)][string]$Bundle,
  [ValidateRange(1, 1000)][int]$Rounds = 1
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$bundlePath = (Resolve-Path -LiteralPath $Bundle).Path
$manifestPath = Join-Path $bundlePath 'bundle-manifest.json'
if ((Get-FileHash -LiteralPath $manifestPath).Hash -ne '377BF4DDA941E7C53C05391203EC5B81C803A543FBC60C5990A816055199E92E') {
  throw 'Unexpected fixed distribution manifest hash'
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.sourceRepository -ne 'pitapa79/DIY_IME' -or $manifest.sourceCommit -ne 'e7f52ff9b510f6f559245cce39727b8e63810da3') {
  throw 'Expected the fixed public Windows distribution'
}
foreach ($file in $manifest.files) {
  $asset = Join-Path $bundlePath $file.path
  if ((Get-Item -LiteralPath $asset).Length -ne $file.bytes -or (Get-FileHash -LiteralPath $asset).Hash -ne $file.sha256) {
    throw 'Input bundle integrity check failed'
  }
}

$output = Join-Path $root ('target/compiled-host-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $output | Out-Null
$names = @('diy-ime2-external-host', 'diy-ime2-node-manager', 'diy-ime2-wasm-worker')
$bins = foreach ($name in $names) { '--bin'; $name }
Push-Location $root
try {
  foreach ($mode in @('reuse', 'jit')) {
    $features = @()
    if ($mode -eq 'reuse') { $features = @('--features', 'compiled-module-probe') }
    cargo build --locked --release --target-dir (Join-Path $root 'target') @features @bins
    if ($LASTEXITCODE -ne 0) { throw "Could not build $mode" }
    $folder = Join-Path $output $mode
    Copy-Item -LiteralPath $bundlePath -Destination $folder -Recurse
    $copy = Get-Content -LiteralPath (Join-Path $folder 'bundle-manifest.json') -Raw | ConvertFrom-Json
    $copy.sourceKind = 'development-worktree'
    $copy.sourceCommit = $null
    foreach ($name in $names) {
      $file = $copy.files | Where-Object path -EQ "bin/$name.exe"
      if ($null -eq $file) { throw 'Missing distribution binary entry' }
      $destination = Join-Path $folder $file.path
      Copy-Item -LiteralPath (Join-Path $root "target/release/$name.exe") -Destination $destination
      $file.bytes = (Get-Item -LiteralPath $destination).Length
      $file.sha256 = (Get-FileHash -LiteralPath $destination).Hash.ToLowerInvariant()
    }
    $copy | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $folder 'bundle-manifest.json') -Encoding utf8
  }

  foreach ($mode in @('jit', 'reuse')) {
    $folder = Join-Path $output $mode
    $report = & (Join-Path $folder 'runtime/node.exe') scripts/benchmark-windows-application.mjs $folder --host-input-rounds $Rounds
    if ($LASTEXITCODE -ne 0) { throw "Host comparison failed for $mode" }
    $report | Set-Content -LiteralPath (Join-Path $output "$mode.json") -Encoding utf8
  }
  Write-Output $output
} finally {
  Pop-Location
}
