param([Parameter(Mandatory)][string]$RuntimeDll,[Parameter(Mandatory)][string]$Wasm,[Parameter(Mandatory)][string]$Probe)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$stage=Join-Path $root ('target/wasm-probes/'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($stage)|Out-Null
function Invoke-ProbeFile([string]$module,[string]$request){
 $output=& $Probe $RuntimeDll $module $request 2> (Join-Path $stage 'stderr.txt')
 if($LASTEXITCODE -ne 0){throw ('Probe failed: '+[IO.File]::ReadAllText((Join-Path $stage 'stderr.txt')))}
 return ($output|ConvertFrom-Json -Depth 100)
}
function Write-Request([object]$request,[string]$name){$p=Join-Path $stage $name;[IO.File]::WriteAllText($p,($request|ConvertTo-Json -Depth 100 -Compress),[Text.UTF8Encoding]::new($false));return $p}
function New-Request([string]$kind,[object]$operation){return @{apiVersion=1;node=@{kind=$kind;version=1};operation=$operation}}
function Text-Value([string]$text){return @{state='data';packet=@{version=1;root=@{type='text';text=$text}}}}
$cases=[Collections.Generic.List[object]]::new()
$baseline=Get-Content -LiteralPath (Join-Path $root 'docs/node-migration-inventory.json') -Raw -ErrorAction SilentlyContinue|ConvertFrom-Json -Depth 100
if($null -eq $baseline){$baseline=Get-Content -LiteralPath (Join-Path $root 'docs/legacy-node-inventory.json') -Raw|ConvertFrom-Json -Depth 100}
$definitions=@($baseline.nodes|Where-Object {$_.classification -ne 'system' -and $_.registered})
foreach($definition in $definitions){$cases.Add((New-Request $definition.definition.kind @{method='describe'}))}
$cases.Add((New-Request 'text.romaji-to-hiragana' @{method='execute';config=@{overrides=@();unknown='preserve'};inputs=@{in=(Text-Value 'kya')};mode='compose'}))
$cases.Add((New-Request 'text.romaji-to-hiragana' @{method='execute';config=@{overrides=@();unknown='preserve'};inputs=@{in=@{state='noOutput'}};mode='compose'}))
$cases.Add((New-Request 'text.dictionary-exact' @{method='execute';config=@{path='dictionary.tsv'};inputs=@{in=(Text-Value 'か')};resources=@{'dictionary.tsv'=@([Text.Encoding]::UTF8.GetBytes("reading`tsurface`tweight`nか`t蚊`t7`n"))}}))
$cases.Add((New-Request 'input.text' @{method='describe'}))
$canary=New-Request 'experimental.packet-passthrough' @{method='execute';config=@{};inputs=@{in=(Text-Value 'private-canary')}};$canary.trusted=$true;$cases.Add($canary)
$large='a'*131072
$cases.Add((New-Request 'experimental.packet-passthrough' @{method='execute';config=@{};inputs=@{in=(Text-Value $large)}}))
$replies=@(Invoke-ProbeFile $Wasm (Write-Request @($cases.ToArray()) 'batch.json'))
if($replies.Count -ne $cases.Count){throw 'Batch reply count mismatch'}
for($i=0;$i -lt $definitions.Count;$i++){
 if(-not $replies[$i].ok -or $replies[$i].data.kind -ne $definitions[$i].definition.kind -or $replies[$i].data.version -ne $definitions[$i].definition.version){throw 'Definition mismatch'}
}
$i=$definitions.Count
if(-not $replies[$i].ok -or $replies[$i].data.out.packet.root.text -ne 'きゃ' -or $replies[$i].data.pending.packet.root.text -ne ''){throw 'Real Wasm romaji mismatch'};$i++
if(-not $replies[$i].ok -or $replies[$i].data.out.state -ne 'noOutput'){throw 'Real Wasm NoOutput mismatch'};$i++
if(-not $replies[$i].ok -or $replies[$i].data.out.packet.root.options[0].value.text -ne '蚊'){throw 'Real Wasm dictionary mismatch'};$i++
if($replies[$i].error.code -ne 'unknown-kind'){throw 'System Node was available in guest'};$i++
if($replies[$i].error.code -ne 'guest-json' -or ($replies[$i]|ConvertTo-Json -Depth 100 -Compress).Contains('private-canary')){throw 'Authority field accepted or echoed'};$i++
if(-not $replies[$i].ok -or $replies[$i].data.out.packet.root.text -ne $large){throw 'Memory-growth roundtrip mismatch'}
$normal=Write-Request (New-Request 'text.romaji-to-hiragana' @{method='describe'}) 'normal.json'
$watBase='(module (memory (export "memory") 1) (func (export "diyime_api_version") (result i32) i32.const 1) (func (export "diyime_alloc") (param i32) (result i32) i32.const 1024) (func (export "diyime_call") (param i32 i32) (result i64) i64.const 0) (func (export "diyime_release_reply")))'
$negative=@{
 'import'='(module (import "wasi_snapshot_preview1" "fd_write" (func)))'
 'version'=$watBase.Replace('i32.const 1)','i32.const 2)')
 'type'=$watBase.Replace('(result i32) i32.const 1)','(result i64) i64.const 1)')
 'allocation'=$watBase.Replace('i32.const 1024','i32.const 65535')
 'descriptor'=$watBase.Replace('i64.const 0','i64.const 429496795135')
 'fuel'=$watBase.Replace('i32.const 1)','(loop $forever br $forever) i32.const 1)')
}
$reasons=@{import='expected.*imports|unknown import|incompatible import';version='guest version unsupported';type='guest result has wrong type';allocation='guest buffer out of bounds';descriptor='guest buffer out of bounds';fuel='fuel'}
foreach($entry in $negative.GetEnumerator()){
 $file=Join-Path $stage ($entry.Key+'.wat');[IO.File]::WriteAllText($file,$entry.Value,[Text.UTF8Encoding]::new($false))
 $output=& $Probe $RuntimeDll $file $normal 2> (Join-Path $stage 'negative-stderr.txt')
 if($LASTEXITCODE -eq 0){throw "Unsafe guest accepted: $($entry.Key)"}
 $reason=[IO.File]::ReadAllText((Join-Path $stage 'negative-stderr.txt'))
 if($reason -notmatch $reasons[$entry.Key]){throw "Wrong refusal for $($entry.Key): $reason"}
}
# A prior failure must not damage a new Store/instance.
$recovered=Invoke-ProbeFile $Wasm $normal;if(-not $recovered.ok){throw 'Normal Store did not recover'}
Write-Output "Real Wasm: $($cases.Count) positive/structured-refusal cases, $($negative.Count) transport/type/import/fuel refusals, normal recovery passed"