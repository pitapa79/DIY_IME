param([string]$Cargo='cargo',[string]$RuntimeRoot,[string]$Node='node')
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if(-not $RuntimeRoot){$RuntimeRoot=Join-Path $root 'target/tools/wasmtime50'}
[IO.Directory]::CreateDirectory($RuntimeRoot)|Out-Null
$archive=Join-Path $RuntimeRoot 'wasmtime-v50.0.1-x86_64-windows-c-api.zip'
if(-not(Test-Path -LiteralPath $archive)){Invoke-WebRequest -Uri 'https://github.com/bytecodealliance/wasmtime/releases/download/v50.0.1/wasmtime-v50.0.1-x86_64-windows-c-api.zip' -OutFile $archive}
if((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne '8b461d9dc12b076d0414b1023433bbb2dc08d3b1c0f0a156853ede35f0147802'){throw 'Official runtime archive hash mismatch'}
$dll=Join-Path $RuntimeRoot 'wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll'
if(-not(Test-Path -LiteralPath $dll)){Expand-Archive -LiteralPath $archive -DestinationPath $RuntimeRoot}
Push-Location $root
try {
 $pattern='(?m)^name = "([^"]+)"\r?\nversion = "([^"]+)"\r?\nsource = "([^"]+)"\r?\nchecksum = "([^"]+)"'
 $known=[Collections.Generic.HashSet[string]]::new()
 foreach($m in [regex]::Matches([IO.File]::ReadAllText((Join-Path $root 'Cargo.lock')),$pattern)){[void]$known.Add($m.Value.Replace([string][char]13,''))}
 foreach($m in [regex]::Matches([IO.File]::ReadAllText((Join-Path $root 'tests/fixtures/wasm_guest/Cargo.lock')),$pattern)){if(-not $known.Contains($m.Value.Replace([string][char]13,''))){throw 'Fixture dependency is not covered by the main audited lockfile'}}
 $metadata=& $Cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
 if($LASTEXITCODE -ne 0){throw 'Cargo metadata failed'}
 $target=$metadata.target_directory
 $fixture='tests/fixtures/wasm_guest/Cargo.toml'
 & $Cargo fmt --manifest-path $fixture -- --check
 if($LASTEXITCODE -ne 0){throw 'Guest fixture fmt failed'}
 & $Cargo build --locked --manifest-path $fixture --release --target wasm32-unknown-unknown --target-dir $target
 if($LASTEXITCODE -ne 0){throw 'Guest fixture build failed; install project wasm32 target'}
 & $Cargo clippy --locked --manifest-path $fixture --target wasm32-unknown-unknown --target-dir $target -- -D warnings
 if($LASTEXITCODE -ne 0){throw 'Guest fixture clippy failed'}
 & $Cargo build --locked --bin diy-ime2-wasm-worker --bin diy-ime2-node-package --bin diy-ime2-node-manager --bin diy-ime2-external-host --example wasm_v3_probe --example wasm_package_probe --example wasm_package_manager_probe --example wasm_permission_probe --example wasm_dictionary_probe --example worker_pipe_probe
 if($LASTEXITCODE -ne 0){throw 'Host Worker build failed'}
 $worker=Join-Path $target 'debug/diy-ime2-wasm-worker.exe'
 $probe=Join-Path $target 'debug/examples/wasm_v3_probe.exe'
 $module=Join-Path $target 'wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm'
 $stderr=Join-Path $target 'unisolated-worker-refusal.txt'
 & $worker 2> $stderr
 if($LASTEXITCODE -eq 0 -or [IO.File]::ReadAllText($stderr) -notmatch 'OS isolation absent'){throw 'Unisolated Worker was not refused'}
 & $probe $worker $dll $module
 if($LASTEXITCODE -ne 0){throw 'Real v3/Wasm isolation integration failed'}
 $oldRangeWorker=$env:DIY_RANGE_WORKER_BINARY
 $oldRangeRuntime=$env:DIY_RANGE_RUNTIME_DLL
 $oldPreparationCrash=$env:DIY_PREPARATION_CRASH_FIXTURE
 try {
  $env:DIY_RANGE_WORKER_BINARY=$worker
  $env:DIY_RANGE_RUNTIME_DLL=$dll
  $env:DIY_PREPARATION_CRASH_FIXTURE=Join-Path $target 'debug/examples/worker_pipe_probe.exe'
  & $Cargo test --locked --lib isolated_worker::runtime::real_jit_ -- --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Real JIT fallback and trap regression failed'}
  & $Cargo test --locked --lib isolated_worker::adapter::ranges::transport_tests::real_appcontainer_ranges -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Real resource range AppContainer integration failed'}
  & $Cargo test --locked --lib isolated_worker::adapter::prepared::tests::real_appcontainer_preparation -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Real artifact preparation AppContainer integration failed'}
  & (Join-Path $PSScriptRoot 'check-resource-sdk.ps1') -Cargo $Cargo -RuntimeDll $dll -Worker $worker
  & (Join-Path $PSScriptRoot 'check-resource-integration-probe.ps1') -Cargo $Cargo -RuntimeDll $dll -Worker $worker
 } finally {
  $env:DIY_RANGE_WORKER_BINARY=$oldRangeWorker
  $env:DIY_RANGE_RUNTIME_DLL=$oldRangeRuntime
  $env:DIY_PREPARATION_CRASH_FIXTURE=$oldPreparationCrash
 }
 & (Join-Path $target 'debug/examples/wasm_package_probe.exe') $worker $dll $module
 if($LASTEXITCODE -ne 0){throw 'Real Wasm package/catalog integration failed'}
 & (Join-Path $target 'debug/examples/wasm_package_manager_probe.exe') (Join-Path $target 'debug/diy-ime2-node-manager.exe') $worker $dll $module
 if($LASTEXITCODE -ne 0){throw 'Real Wasm package manager cancellation/EOF integration failed'}
 & (Join-Path $target 'debug/examples/wasm_permission_probe.exe') $worker $dll $module
 if($LASTEXITCODE -ne 0){throw 'Real Wasm permission/flow/withdrawal integration failed'}
 & $Node (Join-Path $root 'scripts/check-external-host.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-package.exe') $worker $dll $module (Join-Path $root 'tests/contracts/execution_control.json')
 if($LASTEXITCODE -ne 0){throw 'Production external Host execution/consent/EOF integration failed'}
 $oldPreparedJit=$env:DIY_EXPERIMENT_PREPARED_JIT
 try {
  $env:DIY_EXPERIMENT_PREPARED_JIT='1'
  $preparedJitReport=& $Node (Join-Path $root 'scripts/benchmark-wasm-host-scaling.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-package.exe') $worker $dll $module 3 2 single-detailed
  if($LASTEXITCODE -ne 0){throw 'Opt-in prepared JIT Host integration failed'}
  $preparedJit=$preparedJitReport | ConvertFrom-Json
  if($preparedJit.scenarios.Count -ne 1 -or $preparedJit.scenarios[0].preparedWorkers -lt 1){throw 'Opt-in prepared JIT Host never used a prepared Worker'}
  Write-Output "Opt-in prepared JIT Host verified $($preparedJit.scenarios[0].preparedWorkers)/3 warm routes and cleaned all reservations on close"
 } finally {
  $env:DIY_EXPERIMENT_PREPARED_JIT=$oldPreparedJit
 }
 & $Node (Join-Path $root 'scripts/check-wasm-dictionary.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-manager.exe') (Join-Path $target 'debug/diy-ime2-node-package.exe') $worker $dll $module (Join-Path $root 'tests/contracts/execution_control.json') (Join-Path $target 'debug/examples/wasm_dictionary_probe.exe')
 if($LASTEXITCODE -ne 0){throw 'Dictionary broker resource consent/isolation/true-ending integration failed'}
 & $Node (Join-Path $root 'scripts/check-wasm-network.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-package.exe') $worker $dll $module (Join-Path $root 'tests/contracts/execution_control.json')
 if($LASTEXITCODE -ne 0){throw 'Network broker consent/continuation/true-ending integration failed'}
 & $Node (Join-Path $root 'scripts/check-official-bootstrap.mjs') (Join-Path $target 'debug/diy-ime2-node-manager.exe') (Join-Path $target 'debug/diy-ime2-external-host.exe') $worker $dll (Join-Path $root 'distribution/official') (Join-Path $root 'tests/contracts/execution_control.json')
 if($LASTEXITCODE -ne 0){throw 'Official bootstrap/catalog/registered graph integration failed'}
 & $Node (Join-Path $root 'scripts/check-retained-host.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-manager.exe') $worker $dll (Join-Path $root 'distribution/official')
 if($LASTEXITCODE -ne 0){throw 'Ordinary retained Host identity/cancellation/replenishment integration failed'}
 if(Test-Path -LiteralPath (Join-Path $root 'distribution/official/dictionary-producer.wasm')){
  & $Node (Join-Path $root 'scripts/check-retained-host.mjs') (Join-Path $target 'debug/diy-ime2-external-host.exe') (Join-Path $target 'debug/diy-ime2-node-manager.exe') $worker $dll (Join-Path $root 'distribution/official') --single-worker
  if($LASTEXITCODE -ne 0){throw 'Prepared dictionary single Worker integration failed'}
 }
 $taskPreviousWorker=$env:DIY_RANGE_WORKER_BINARY
 $taskPreviousDll=$env:DIY_RANGE_RUNTIME_DLL
 $taskPreviousFixture=$env:DIY_RETAINED_HOST_FIXTURE_WASM
 try {
  $env:DIY_RANGE_WORKER_BINARY=$worker
  $env:DIY_RANGE_RUNTIME_DLL=$dll
  $env:DIY_RETAINED_HOST_FIXTURE_WASM=$module
  & $Cargo test --locked --lib isolated_worker::adapter::tests::real_prepared_jit_uses_production_worker_lifecycle -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Prepared JIT production Worker lifecycle failed'}
  & $Cargo test --locked --lib isolated_worker::adapter::retained::tests::real_core_skipped_suffix -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Retained coordinator actual Core Skipped suffix integration failed'}
 } finally {
  $env:DIY_RANGE_WORKER_BINARY=$taskPreviousWorker
  $env:DIY_RANGE_RUNTIME_DLL=$taskPreviousDll
  $env:DIY_RETAINED_HOST_FIXTURE_WASM=$taskPreviousFixture
 }
} finally {Pop-Location}
