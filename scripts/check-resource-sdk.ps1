param([string]$Cargo='cargo',[Parameter(Mandatory=$true)][string]$RuntimeDll,[string]$Worker)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $root
try {
 $metadata=& $Cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
 if($LASTEXITCODE -ne 0){throw 'SDK Cargo metadata failed'}
 $target=$metadata.target_directory
 $fixture='tests/fixtures/resource_sdk_guest/Cargo.toml'
 $pattern='(?m)^name = "([^"]+)"\r?\nversion = "([^"]+)"\r?\nsource = "([^"]+)"\r?\nchecksum = "([^"]+)"'
 $known=[Collections.Generic.HashSet[string]]::new()
 foreach($m in [regex]::Matches([IO.File]::ReadAllText((Join-Path $root 'Cargo.lock')),$pattern)){[void]$known.Add($m.Value.Replace([string][char]13,''))}
 foreach($m in [regex]::Matches([IO.File]::ReadAllText((Join-Path $root 'tests/fixtures/resource_sdk_guest/Cargo.lock')),$pattern)){if(-not $known.Contains($m.Value.Replace([string][char]13,''))){throw 'SDK fixture dependency is outside the main audited lockfile'}}
 & $Cargo test --locked -p diy-ime2-wasm-resource-sdk --all-features
 if($LASTEXITCODE -ne 0){throw 'SDK feature tests failed'}
 & $Cargo test --locked -p diy-ime2-wasm-resource-sdk --no-default-features
 if($LASTEXITCODE -ne 0){throw 'SDK minimal tests failed'}
 & $Cargo fmt --manifest-path $fixture -- --check
 if($LASTEXITCODE -ne 0){throw 'SDK guest fmt failed'}
 $stage=Join-Path $target 'sdk-fixtures'
 [IO.Directory]::CreateDirectory($stage)|Out-Null
 $built=Join-Path $target 'wasm32-unknown-unknown/release/diyime2_resource_sdk_fixture.wasm'
 $producer=Join-Path $stage 'producer.wasm'
 $consumer=Join-Path $stage 'consumer.wasm'
 & $Cargo build --locked --manifest-path $fixture --release --target wasm32-unknown-unknown --target-dir $target --features producer
 if($LASTEXITCODE -ne 0){throw 'SDK producer build failed'}
 Copy-Item -LiteralPath $built -Destination $producer -Force
 & $Cargo clippy --locked --manifest-path $fixture --target wasm32-unknown-unknown --target-dir $target --features producer -- -D warnings
 if($LASTEXITCODE -ne 0){throw 'SDK producer clippy failed'}
 & $Cargo build --locked --manifest-path $fixture --release --target wasm32-unknown-unknown --target-dir $target --no-default-features
 if($LASTEXITCODE -ne 0){throw 'SDK consumer build failed'}
 Copy-Item -LiteralPath $built -Destination $consumer -Force
 & $Cargo clippy --locked --manifest-path $fixture --target wasm32-unknown-unknown --target-dir $target --no-default-features -- -D warnings
 if($LASTEXITCODE -ne 0){throw 'SDK consumer clippy failed'}
 if(-not $Worker){
  & $Cargo build --locked --bin diy-ime2-wasm-worker
  if($LASTEXITCODE -ne 0){throw 'SDK native Worker build failed'}
  $Worker=Join-Path $target 'debug/diy-ime2-wasm-worker.exe'
 }
 $oldWorker=$env:DIY_RANGE_WORKER_BINARY
 $oldRuntime=$env:DIY_RANGE_RUNTIME_DLL
 $oldProducer=$env:DIY_RESOURCE_SDK_PRODUCER_WASM
 $oldConsumer=$env:DIY_RESOURCE_SDK_CONSUMER_WASM
 try {
  $env:DIY_RANGE_WORKER_BINARY=$Worker
  $env:DIY_RANGE_RUNTIME_DLL=$RuntimeDll
  $env:DIY_RESOURCE_SDK_PRODUCER_WASM=$producer
  $env:DIY_RESOURCE_SDK_CONSUMER_WASM=$consumer
  & $Cargo test --locked --lib isolated_worker::adapter::resource_sdk_tests::real_resource_sdk_guests -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'SDK actual Wasm/AppContainer integration failed'}
  & $Cargo test --locked --lib isolated_worker::adapter::retained_tests::real_retained_module_loop -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Retained module/Store lifetime integration failed'}
  & $Cargo test --locked --lib isolated_worker::adapter::retained_driver_tests:: -- --include-ignored --nocapture --test-threads=1
  if($LASTEXITCODE -ne 0){throw 'Retained bounded driver/cancellation/fault integration failed'}
  & $Cargo test --locked --lib isolated_worker::package::official::fixed_pipeline::tests::real_official_fixed_review -- --exact --ignored --nocapture
  if($LASTEXITCODE -ne 0){throw 'Official fixed pipeline static review integration failed'}
 } finally {
  $env:DIY_RANGE_WORKER_BINARY=$oldWorker
  $env:DIY_RANGE_RUNTIME_DLL=$oldRuntime
  $env:DIY_RESOURCE_SDK_PRODUCER_WASM=$oldProducer
  $env:DIY_RESOURCE_SDK_CONSUMER_WASM=$oldConsumer
 }
} finally {Pop-Location}
