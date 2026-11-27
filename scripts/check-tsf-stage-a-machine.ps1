param(
    [Parameter(Mandatory)][string]$BuildDirectory,
    [string]$Cargo = 'cargo',
    [string]$StageBHostConfiguration,
    [ValidateRange(0, 300)][int]$ManualAppSeconds = 0,
    [switch]$WaitForManualReady,
    [switch]$ReuseMatchingStaging,
    [switch]$FreshStaging,
    [switch]$ManualUntilStopped,
    [switch]$ManualOnly,
    [string]$TestExecutable,
    [string]$HostProbeExecutable,
    [switch]$CheckPrerequisites
)
$ErrorActionPreference = 'Stop'
if ($ManualOnly -and !$ManualUntilStopped) {
    throw 'ManualOnly requires ManualUntilStopped.'
}
if ($ManualUntilStopped -and (!$StageBHostConfiguration -or $ManualAppSeconds -gt 0 -or $WaitForManualReady)) {
    throw 'ManualUntilStopped requires a Stage B configuration and uses its own completion signal.'
}
if ($FreshStaging -and $ReuseMatchingStaging) {
    throw 'Choose either fresh staging or reuse of matching staging.'
}
if (![Environment]::Is64BitProcess -or ![Environment]::UserInteractive) {
    throw 'Run on an interactive x64 Windows desktop.'
}
if ($ManualAppSeconds -gt 0 -and !$StageBHostConfiguration) {
    throw 'Manual application checks require a Stage B Host configuration.'
}
if ($WaitForManualReady -and $ManualAppSeconds -eq 0) {
    throw 'Waiting for manual readiness requires a manual application check duration.'
}

$service = '{EECF857D-82EB-4C57-9592-257F6ECF62F1}'
$profile = '8c73cb26-1ccf-45e1-a259-f29b674219e0'
$build = (Resolve-Path -LiteralPath $BuildDirectory).Path
$tool = Join-Path $build 'diy-ime2-tsf-test-profile.exe'
$dll = Join-Path $build 'diy_ime2_tsf.dll'
$machineInstaller = Join-Path $PSScriptRoot 'test-tsf-stage-a-registration.ps1'
$systemPowerShell = Join-Path $env:WINDIR 'System32/WindowsPowerShell/v1.0/powershell.exe'
. (Join-Path $PSScriptRoot 'tsf-stage-a-staging.ps1')
$runId = [guid]::NewGuid()
$staging = Get-TsfStagingDirectory $runId $FreshStaging.IsPresent
$installerStatus = Join-Path $staging 'test-status.txt'

function Get-LanguageSettings {
    $query = '$ProgressPreference = "SilentlyContinue"; (Get-WinDefaultInputMethodOverride).InputMethodTip; foreach ($language in Get-WinUserLanguageList) { foreach ($method in $language.InputMethodTips) { $method } }'
    $settings = & $systemPowerShell -NonInteractive -NoProfile -Command $query
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the input-method settings.' }
    return $settings
}

function Get-TestRegistration {
    & $tool status
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the TSF profile status.' }
}

function Test-DedicatedKeys {
    foreach ($hive in 'HKCU', 'HKLM') {
        foreach ($key in "Software\Classes\CLSID\$service", "Software\Microsoft\CTF\TIP\$service") {
            if (Test-Path -LiteralPath "Registry::$hive\$key") { return $true }
        }
    }
    return $false
}

function Wait-TsfManualReady(
    [Diagnostics.Process]$Installer,
    [string]$Signal,
    [int]$TimeoutSeconds
) {
    $wait = [Diagnostics.Stopwatch]::StartNew()
    while ($wait.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        if ($Installer.HasExited) { throw 'The machine registration ended while waiting for IME selection.' }
        if ([IO.File]::Exists($Signal)) { return }
        Start-Sleep -Milliseconds 250
    }
    throw 'IME selection was not confirmed within the allowed time.'
}

$requiredFiles = @($tool, $dll)
if ($StageBHostConfiguration) {
    $StageBHostConfiguration = (Resolve-Path -LiteralPath $StageBHostConfiguration).Path
    $stageBHost = Join-Path $build 'diy-ime2-external-host.exe'
    $requiredFiles += @($StageBHostConfiguration, $stageBHost)
}
foreach ($file in $requiredFiles) {
    if (!(Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing test file $file" }
}
$requiredSpace = ($requiredFiles | ForEach-Object { (Get-Item -LiteralPath $_).Length } | Measure-Object -Sum).Sum
$drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($staging))
if ($drive.AvailableFreeSpace -lt ($requiredSpace + 64MB)) {
    throw "Insufficient staging space. Need $requiredSpace bytes plus 64 MiB for test records."
}
$languageBaseline = @(Get-LanguageSettings)
if ((Test-DedicatedKeys) -or
    ($languageBaseline -match $profile) -or ((Get-TestRegistration) -notmatch 'registered=false enabled_for_current_user=false')) {
    throw 'A previous test profile is still present. Use a clean baseline.'
}
$hashes = @{
    'diy-ime2-tsf-test-profile.exe' = (Get-FileHash -LiteralPath $tool).Hash
    'diy_ime2_tsf.dll' = (Get-FileHash -LiteralPath $dll).Hash
}
if ($StageBHostConfiguration) {
    $hashes['diy-ime2-external-host.exe'] = (Get-FileHash -LiteralPath $stageBHost).Hash
    $hashes['ime-execution.json'] = (Get-FileHash -LiteralPath $StageBHostConfiguration).Hash
}
if (Test-Path -LiteralPath $staging) {
    if (!$ReuseMatchingStaging) { throw 'A previous TSF staging directory remains.' }
    Assert-ReusableTsfStaging $staging $hashes
    'Previous TSF staging matches this test build and configuration.'
}
if ($CheckPrerequisites) {
    'The compiled test files and clean registration baseline were found.'
    return
}

$repo = Split-Path $PSScriptRoot -Parent
function Invoke-HostProbe([string]$HostPath, [string]$ConfigurationPath) {
    if ($HostProbeExecutable) {
        & $HostProbeExecutable $HostPath $ConfigurationPath
    } else {
        & $Cargo run --locked -p diy-ime2-ime-host --example ime_external_host_probe -- $HostPath $ConfigurationPath
    }
    if ($LASTEXITCODE -ne 0) { throw 'The Stage B Host failed the synthetic fixture check.' }
}
if ($TestExecutable) { $TestExecutable = (Resolve-Path -LiteralPath $TestExecutable).Path }
if ($HostProbeExecutable) { $HostProbeExecutable = (Resolve-Path -LiteralPath $HostProbeExecutable).Path }
Push-Location -LiteralPath $repo
try {
    if ($StageBHostConfiguration) {
        Invoke-HostProbe $stageBHost $StageBHostConfiguration
    }
    if (!$TestExecutable -and !$ManualOnly) {
        $events = & $Cargo test --locked -p diy-ime2-tsf --test stage_a --no-run --message-format=json
        if ($LASTEXITCODE -ne 0) { throw 'The Win32 test executable could not be built.' }
    }
} finally { Pop-Location }
$executables = @(
    if ($TestExecutable) { $TestExecutable }
    foreach ($event in $events) {
        $item = $event | ConvertFrom-Json
        if ($item.reason -eq 'compiler-artifact' -and $item.target.name -eq 'stage_a' -and $item.executable) {
            $item.executable
        }
    }
)
if (!$ManualOnly -and $executables.Count -ne 1) { throw 'Expected exactly one Win32 test executable.' }

$session = Join-Path ([IO.Path]::GetTempPath()) ('diy-ime-tsf-' + $runId.ToString('N'))
[void][IO.Directory]::CreateDirectory($session)
$release = Join-Path $session 'release.signal'
$ready = Join-Path $session 'ready.signal'
$finish = Join-Path $session 'finish.signal'
$languageBaseline | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $session 'input-baseline.json') -Encoding utf8
$toolHash = $hashes['diy-ime2-tsf-test-profile.exe']
$dllHash = $hashes['diy_ime2_tsf.dll']
$readyWaitSeconds = if ($WaitForManualReady) { 300 } else { 0 }
$seconds = if ($StageBHostConfiguration) { 300 + $readyWaitSeconds + $ManualAppSeconds } else { 150 }
$arguments = @(
    '-NoProfile', '-File', ('"' + $machineInstaller + '"'),
    '-BuildDirectory', ('"' + $build + '"'), '-ReleaseFile', ('"' + $release + '"'),
    '-ProfileToolSha256', $toolHash, '-ServiceDllSha256', $dllHash, '-Seconds', $seconds.ToString(),
    '-RunId', $runId.ToString()
)
if ($StageBHostConfiguration) {
    $arguments += @(
        '-StageBHostConfiguration', ('"' + $StageBHostConfiguration + '"'),
        '-StageBConfigSha256', $hashes['ime-execution.json'],
        '-StageBHostSha256', $hashes['diy-ime2-external-host.exe']
    )
}
if ($ReuseMatchingStaging) { $arguments += '-ReuseMatchingStaging' }
if ($FreshStaging) { $arguments += '-FreshStaging' }
if ($ManualUntilStopped) {
    $controller = Get-Process -Id $PID
    $arguments += @('-ControllerProcessId', $PID.ToString(), '-ControllerStartTicks',
        $controller.StartTime.ToUniversalTime().Ticks.ToString())
    $controller.Dispose()
}
$installer = $null
$enableAttempted = $false
$cleanupVerified = $false
try {
    $installer = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $registrationTimeout = 45
    $installed = $false
    do {
        if (Test-Path -LiteralPath $installerStatus -PathType Leaf) {
            $report = @(Get-Content -LiteralPath $installerStatus -ErrorAction SilentlyContinue)
            if ($report -contains "run $runId") {
                $installerError = $report | Where-Object { $_ -like 'error: *' } | Select-Object -Last 1
                if ($installerError) { throw "Temporary registration failed: $installerError" }
                $installed = ($report -contains 'installed') -and ((Get-TestRegistration) -match 'registered=true')
                if ($installed) { break }
            }
        }
        Start-Sleep -Milliseconds 250
    } while (!$installer.HasExited -and $clock.Elapsed.TotalSeconds -lt $registrationTimeout)
    if (!$installed) {
        if ($installer.HasExited) { throw "The registration helper exited before verification (code $($installer.ExitCode))." }
        throw "The machine profile could not be verified within $registrationTimeout seconds."
    }
    if ($StageBHostConfiguration) {
        Push-Location -LiteralPath $repo
        try {
            Invoke-HostProbe (Join-Path $staging 'diy-ime2-external-host.exe') (Join-Path $staging 'ime-execution.json')
        } finally { Pop-Location }
    }
    $enableAttempted = $true
    & $tool enable
    if ($LASTEXITCODE -ne 0) { throw 'The current user could not enable the dedicated profile.' }
    if ((Get-TestRegistration) -notmatch 'registered=true enabled_for_current_user=true') {
        throw 'The current-user TSF profile did not become enabled.'
    }
    $tests = @()
    if ($ManualOnly) {
        'Registered Win32 app tests skipped for this manual-only run.'
    } else {
        $tests = @('interactive_machine_registered_app_routes_input')
        if ($StageBHostConfiguration) {
            $tests += @('interactive_machine_registered_stage_b','interactive_machine_registered_stage_b_protection')
        }
    }
    $failures = [System.Collections.Generic.List[string]]::new()
    foreach ($test in $tests) {
        & $executables[0] --ignored --exact $test --nocapture
        if ($LASTEXITCODE -ne 0) { $failures.Add($test) }
    }
    if ($ManualUntilStopped) {
        'Manual application checks are ready until explicitly stopped.'
        "Manual application completion signal: $finish"
        'Return to the original input method and close test applications before completing.'
        while (![IO.File]::Exists($finish)) {
            if ($installer.HasExited) { throw 'The registration helper ended during manual checks.' }
            Start-Sleep -Milliseconds 300
        }
    } elseif ($ManualAppSeconds -gt 0) {
        $manualStartDeadline = $seconds - $ManualAppSeconds - 15
        if ($WaitForManualReady) {
            $remaining = [int][Math]::Floor($manualStartDeadline - $clock.Elapsed.TotalSeconds)
            $selectionTimeout = [Math]::Min($readyWaitSeconds, $remaining)
            if ($selectionTimeout -le 0) { throw 'Too little registration time remains to confirm IME selection.' }
            "Select the test IME and confirm readiness within $selectionTimeout seconds."
            "Manual application readiness signal: $ready"
            Wait-TsfManualReady $installer $ready $selectionTimeout
        }
        if ($installer.HasExited -or $clock.Elapsed.TotalSeconds -ge $manualStartDeadline) {
            throw 'Too little registration time remains for the manual application checks.'
        }
        "Manual application checks are ready for $ManualAppSeconds seconds."
        'Before this window ends, select the original input method and close the Notepad and browser windows opened for this test.'
        if ($installer.WaitForExit(1000 * $ManualAppSeconds)) {
            throw 'The machine registration ended during the manual application checks.'
        }
    }
    if ($failures.Count -ne 0) { throw "The Win32 TSF tests failed: $($failures -join ', ')" }
} finally {
    try {
        if ($enableAttempted) {
            try {
                & $tool disable
                if ($LASTEXITCODE -ne 0) { Write-Warning 'The TSF API did not disable the test profile.' }
            } finally {
                $user = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', 'Registry64')
                try {
                    $tips = $user.OpenSubKey('Software\Microsoft\CTF\TIP', $true)
                    if ($tips) {
                        try { $tips.DeleteSubKeyTree($service, $false) } finally { $tips.Dispose() }
                    }
                } finally { $user.Dispose() }
            }
        }
    } finally {
        try {
            if ($installer) {
                [IO.File]::WriteAllText($release, 'release')
                if (!$installer.WaitForExit(20000)) { throw 'Machine registration cleanup has not exited.' }
                # The helper's final disable/remove calls can recreate the user key.
                if ($enableAttempted) {
                    $user = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', 'Registry64')
                    try {
                        $tips = $user.OpenSubKey('Software\Microsoft\CTF\TIP', $true)
                        if ($tips) {
                            try { $tips.DeleteSubKeyTree($service, $false) } finally { $tips.Dispose() }
                        }
                    } finally { $user.Dispose() }
                }
                $clock = [Diagnostics.Stopwatch]::StartNew()
                do {
                    $status = Get-TestRegistration
                    $languageAfter = @(Get-LanguageSettings)
                    $clean = ($status -match 'registered=false enabled_for_current_user=false') -and
                        !($languageAfter -match $profile) -and !(Test-DedicatedKeys)
                    if (!$clean) { Start-Sleep -Milliseconds 300 }
                } while (!$clean -and $clock.Elapsed.TotalSeconds -lt 8)
                if (!$clean) { throw 'A test registration remains after cleanup.' }
                if (($languageBaseline -join "`n") -cne ($languageAfter -join "`n")) {
                    throw 'The input-method or default override settings changed from the baseline.'
                }
                'The test profile is absent. The original input-method settings match.'
                $cleanupVerified = $true
                if (Test-Path -LiteralPath $staging) {
                    throw 'The test staging files remain after cleanup.'
                }
                'The staged files are absent.'
            }
        } finally {
            if (!$installer -or $installer.HasExited) {
                [IO.File]::Delete($release)
                [IO.File]::Delete($ready)
                [IO.File]::Delete($finish)
                if ($cleanupVerified -or !$installer) {
                    [IO.File]::Delete((Join-Path $session 'input-baseline.json'))
                    [IO.Directory]::Delete($session)
                } else {
                    Write-Warning "Cleanup verification failed. Input settings baseline retained in $session"
                }
            }
            if ($installer) { $installer.Dispose() }
        }
    }
}
