param([string]$Cargo='cargo',[Parameter(Mandatory=$true)][string]$RuntimeDll,[string]$Worker,[string]$Producer,[string]$Consumer)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $root
try {
    $metadata=& $Cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if($LASTEXITCODE -ne 0){throw 'Probe metadata failed'}
    $target=$metadata.target_directory
    & $Cargo build --locked --features resource-integration-probe --example resource_integration_probe --bin diy-ime2-wasm-worker
    if($LASTEXITCODE -ne 0){throw 'Integration probe build failed'}
    & $Cargo clippy --locked --features resource-integration-probe --example resource_integration_probe -- -D warnings
    if($LASTEXITCODE -ne 0){throw 'Integration probe clippy failed'}
    if(-not $Worker){$Worker=Join-Path $target 'debug/diy-ime2-wasm-worker.exe'}
    if(-not $Producer){$Producer=Join-Path $target 'sdk-fixtures/producer.wasm'}
    if(-not $Consumer){$Consumer=Join-Path $target 'sdk-fixtures/consumer.wasm'}
    $stage=Join-Path $target 'resource-integration-fixtures'
    [IO.Directory]::CreateDirectory($stage)|Out-Null
    $source=Join-Path $stage 'synthetic.bin'
    $bytes=[byte[]](0,255,0xe6,0xbc,0xa2,10,13,42)
    [IO.File]::WriteAllBytes($source,$bytes)
    $manifest=Join-Path $stage 'manifest.json'
    $spec=[ordered]@{workspace=$root;worker=$Worker;runtime=$RuntimeDll;producer=$Producer;consumer=$Consumer;source=$source;sourceReference='source.bin';node=@{kind='fixture.resource';version=1};config=@{};cases=@(@{inputs=@{};mode='finalize'},@{inputs=@{};cancelBeforeStart=$true})}
    [IO.File]::WriteAllText($manifest,($spec|ConvertTo-Json -Depth 20),[Text.UTF8Encoding]::new($false))
    $probe=Join-Path $target 'debug/examples/resource_integration_probe.exe'
    $raw=& $probe $manifest
    if($LASTEXITCODE -ne 0){throw 'Actual broker probe failed'}
    $result=$raw|ConvertFrom-Json
    if($result.syntheticScope -ne $true -or $result.preparation.actualExitConfirmed -ne $true -or $result.preparation.exitCode -ne 0 -or $result.artifact.bytes -ne $bytes.Length){throw 'Preparation evidence missing'}
    $normal=$result.cases[0]
    if($normal.receipt.actualExitConfirmed -ne $true -or $normal.receipt.exitCode -ne 0 -or $normal.receipt.reason -ne 'Exited' -or $normal.receipt.inputDelivered -ne $true -or (($normal.data.copied -join ',') -ne ($bytes -join ','))){throw 'Actual consumer bytes/receipt mismatch'}
    if($result.cases[1].cancelledBeforeStart -ne $true -or $result.cases[1].processStarted -ne $false){throw 'Cancelled input started a Worker'}
    [IO.File]::WriteAllText((Join-Path $stage 'success-receipt.json'),$raw,[Text.UTF8Encoding]::new($false))
    $spec.producer=$Consumer
    [IO.File]::WriteAllText($manifest,($spec|ConvertTo-Json -Depth 20),[Text.UTF8Encoding]::new($false))
    $raw=& $probe $manifest
    if($LASTEXITCODE -ne 0){throw 'Wrong-role probe did not reach a true exit receipt'}
    $rejected=$raw|ConvertFrom-Json
    if($rejected.preparation.actualExitConfirmed -ne $true -or -not $rejected.preparationError -or $rejected.artifact -or $rejected.cases.Count -ne 0){throw 'Wrong-role module produced an adopted artifact'}
    [IO.File]::WriteAllText((Join-Path $stage 'refusal-receipt.json'),$raw,[Text.UTF8Encoding]::new($false))
    Write-Output 'Actual resource broker: binary roundtrip, pre-start cancellation and wrong-role artifact refusal passed with true exits'
} finally {Pop-Location}
