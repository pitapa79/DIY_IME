param(
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [Parameter(Mandatory=$true)][string]$NodeRuntimeRoot,
 [Parameter(Mandatory=$true)][string]$WasmtimeRoot,
 [string]$SourceCommit,
 [string]$Cargo='cargo'
)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$node=Join-Path $NodeRuntimeRoot 'node.exe'
$dll=Join-Path $WasmtimeRoot 'lib/wasmtime.dll'
if((Get-FileHash -LiteralPath $node -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'd702daa815eabbff763c7d3808e235cf031cf2ca70211b56847bc72f51e240e5'){throw 'Pinned Node24.22.1 Windows x64 executable differs'}
if((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'fa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef'){throw 'Pinned Wasmtime50.0.1 Windows C API differs'}
if($SourceCommit -and $SourceCommit -notmatch '^[0-9a-f]{40}$'){throw 'Full source commit required'}
$out=[IO.Path]::GetFullPath($OutputDirectory)
$parent=[IO.Path]::GetDirectoryName($out)
if(-not(Test-Path -LiteralPath $parent -PathType Container)){throw 'Create the selected output parent first'}
$parent=(Get-Item -LiteralPath $parent).FullName
$out=Join-Path $parent ([IO.Path]::GetFileName($out))
if(Test-Path -LiteralPath $out){throw 'Output already exists; refusing overwrite'}
$archive=$out+'.zip'
if(Test-Path -LiteralPath $archive){throw 'Output archive already exists'}
$stage=Join-Path $parent ('.diyime-bundle-'+[Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($stage)|Out-Null
$vars=@('CARGO_TARGET_DIR','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','VITE_DISTRIBUTION')
$saved=@{};foreach($v in $vars){$saved[$v]=[Environment]::GetEnvironmentVariable($v,'Process')}
try{
 Push-Location $root
 try{
  $metadata=& $Cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
  if($LASTEXITCODE -ne 0){throw 'Cargo metadata failed'}
  $target=Join-Path $metadata.target_directory 'windows-distribution'
  $env:CARGO_TARGET_DIR=$target
  $flags=@('--remap-path-prefix='+$root+'=source')
  $cargoSourceHome=if($env:CARGO_HOME){$env:CARGO_HOME}else{Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo'}
  $flags+=('--remap-path-prefix='+(Join-Path $cargoSourceHome 'registry')+'=registry')
  $env:RUSTFLAGS=$null;$env:CARGO_ENCODED_RUSTFLAGS=$flags -join [char]31
  & $Cargo build --locked --release --bin diy-ime2-app --bin diy-ime2-wasm-worker --bin diy-ime2-node-manager --bin diy-ime2-external-host --bin diy-ime2
  if($LASTEXITCODE -ne 0){throw 'Release application build failed'}
  $env:VITE_DISTRIBUTION='1'
  Push-Location (Join-Path $root 'editor')
  try{
   & $node node_modules/typescript/bin/tsc --noEmit
   if($LASTEXITCODE -ne 0){throw 'Editor types failed'}
   & $node node_modules/vite/bin/vite.js build --mode distribution
   if($LASTEXITCODE -ne 0){throw 'Static Editor build failed'}
   & $node node_modules/vite/bin/vite.js build --config vite.server.config.ts
   if($LASTEXITCODE -ne 0){throw 'Application server build failed'}
  }finally{Pop-Location}
  foreach($dir in @('bin','runtime','web','official','defaults','examples/graphs','notices')){[IO.Directory]::CreateDirectory((Join-Path $stage $dir))|Out-Null}
  Copy-Item -LiteralPath (Join-Path $target 'release/diy-ime2-app.exe') -Destination (Join-Path $stage 'DIY_IME2.exe')
  foreach($name in @('diy-ime2','diy-ime2-wasm-worker','diy-ime2-node-manager','diy-ime2-external-host')){Copy-Item -LiteralPath (Join-Path $target ('release/'+$name+'.exe')) -Destination (Join-Path $stage ('bin/'+$name+'.exe'))}
  Copy-Item -LiteralPath $node -Destination (Join-Path $stage 'runtime/node.exe')
  Copy-Item -LiteralPath $dll -Destination (Join-Path $stage 'runtime/wasmtime.dll')
  Copy-Item -LiteralPath (Join-Path $NodeRuntimeRoot 'LICENSE') -Destination (Join-Path $stage 'notices/Node-LICENSE.txt')
  Copy-Item -LiteralPath (Join-Path $WasmtimeRoot 'LICENSE') -Destination (Join-Path $stage 'notices/Wasmtime-LICENSE.txt')
  Copy-Item -LiteralPath (Join-Path $root 'editor/dist-server/server.mjs') -Destination (Join-Path $stage 'runtime/server.mjs')
  foreach($entry in Get-ChildItem -LiteralPath (Join-Path $root 'editor/dist') -Force){Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $stage 'web') -Recurse}
  foreach($entry in Get-ChildItem -LiteralPath (Join-Path $root 'distribution/official') -Force){Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $stage 'official') -Recurse}
  Copy-Item -LiteralPath (Join-Path $root 'distribution/defaults/dictionary.tsv') -Destination (Join-Path $stage 'defaults/dictionary.tsv')
  foreach($entry in Get-ChildItem -LiteralPath (Join-Path $root 'examples/graphs') -File -Filter '*.json'){Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $stage 'examples/graphs')}
  Copy-Item -LiteralPath (Join-Path $root 'distribution/README-windows.txt') -Destination (Join-Path $stage 'README.txt')
  # Preserve original license/notice files of the bundled browser dependencies.
  $lock=Get-Content (Join-Path $root 'editor/package-lock.json') -Raw | ConvertFrom-Json -AsHashtable
  foreach($record in $lock.packages.GetEnumerator()){
   if(-not $record.Key.StartsWith('node_modules/') -or $record.Value.dev){continue}
   $name=$record.Key.Substring(13)
   $source=Join-Path (Join-Path $root 'editor/node_modules') $name
   if(-not(Test-Path -LiteralPath $source -PathType Container)){continue}
   $destination=Join-Path $stage ('notices/editor/'+$name)
   [IO.Directory]::CreateDirectory($destination)|Out-Null
   foreach($entry in Get-ChildItem -LiteralPath $source -File | Where-Object {$_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)'}){Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $destination $entry.Name)}
  }
  # Native dependencies use the exact locked source already present for this build.
  $pattern='(?m)^name = "([^"]+)"\r?\nversion = "([^"]+)"'
  $cargoSourceHome=if($env:CARGO_HOME){$env:CARGO_HOME}else{Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo'}
  $registryRoots=@(Get-ChildItem -LiteralPath (Join-Path $cargoSourceHome 'registry/src') -Directory)
  foreach($match in [regex]::Matches([IO.File]::ReadAllText((Join-Path $root 'Cargo.lock')),$pattern)){
   $name=$match.Groups[1].Value+'-'+$match.Groups[2].Value
   foreach($registry in $registryRoots){$source=Join-Path $registry.FullName $name;if(-not(Test-Path -LiteralPath $source -PathType Container)){continue};$destination=Join-Path $stage ('notices/native/'+$name);[IO.Directory]::CreateDirectory($destination)|Out-Null;foreach($entry in Get-ChildItem -LiteralPath $source -File | Where-Object {$_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)'}){Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $destination $entry.Name)}}
  }
  $files=@(Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName | ForEach-Object { @{path=$_.FullName.Substring($stage.Length+1).Replace('\','/');bytes=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()} })
  $pin=Get-Content (Join-Path $root 'distribution/official/pin.json') -Raw | ConvertFrom-Json
  $receipt=@{formatVersion=1;sourceRepository='pitapa79/DIY_IME2';sourceCommit=$(if($SourceCommit){$SourceCommit}else{$null});sourceKind=$(if($SourceCommit){'pinned-pipeline'}else{'development-worktree'});nodeVersion='24.22.1';wasmtimeVersion='50.0.1';official=$pin;files=$files}
  $receipt | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $stage 'bundle-manifest.json') -Encoding utf8
  [IO.Directory]::Move($stage,$out)
  Compress-Archive -LiteralPath $out -DestinationPath $archive -CompressionLevel Optimal
  Write-Output ('Windows application: '+$out)
  Write-Output ('Archive SHA256: '+(Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant())
 }finally{Pop-Location}
}finally{
 foreach($v in $vars){[Environment]::SetEnvironmentVariable($v,$saved[$v],'Process')}
 # Keep a uniquely owned failed stage for inspection, without recursive cleanup of caller paths.
}