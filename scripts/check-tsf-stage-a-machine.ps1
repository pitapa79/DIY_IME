param(
    [Parameter(Mandatory)][string]$BuildDirectory,
    [string]$Cargo = 'cargo',
    [string]$StageBHostConfiguration,
    [ValidateRange(0, 240)][int]$ManualAppSeconds = 0,
    [switch]$CheckPrerequisites
)
$ErrorActionPreference = 'Stop'
if (![Environment]::Is64BitProcess -or ![Environment]::UserInteractive) {
    throw 'Run on an interactive x64 Windows desktop.'
}
if ($ManualAppSeconds -gt 0 -and !$StageBHostConfiguration) {
    throw 'Manual application checks require a Stage B Host configuration.'
}

$service = '{EECF857D-82EB-4C57-9592-257F6ECF62F1}'
$profile = '8c73cb26-1ccf-45e1-a259-f29b674219e0'
$build = (Resolve-Path -LiteralPath $BuildDirectory).Path
$tool = Join-Path $build 'diy-ime2-tsf-test-profile.exe'
$dll = Join-Path $build 'diy_ime2_tsf.dll'
$staging = Join-Path $env:ProgramFiles "DIY-IME-Stage-A-$service"
$installerStatus = Join-Path $staging 'test-status.txt'
$machineInstaller = Join-Path $PSScriptRoot 'test-tsf-stage-a-registration.ps1'
$systemPowerShell = Join-Path $env:WINDIR 'System32/WindowsPowerShell/v1.0/powershell.exe'

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

$requiredFiles = @($tool, $dll)
if ($StageBHostConfiguration) {
    $StageBHostConfiguration = (Resolve-Path -LiteralPath $StageBHostConfiguration).Path
    $stageBHost = Join-Path $build 'diy-ime2-external-host.exe'
    $requiredFiles += @($StageBHostConfiguration, $stageBHost)
}
foreach ($file in $requiredFiles) {
    if (!(Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing test file $file" }
}
$languageBaseline = @(Get-LanguageSettings)
if ((Test-DedicatedKeys) -or (Test-Path -LiteralPath $staging) -or
    ($languageBaseline -match $profile) -or ((Get-TestRegistration) -notmatch 'registered=false enabled_for_current_user=false')) {
    throw 'A previous test profile is still present. Use a clean baseline.'
}
if ($CheckPrerequisites) {
    'The compiled test files and clean registration baseline were found.'
    return
}

# Compile before triggering UAC so the registration window is used only for verification.
$repo = Split-Path $PSScriptRoot -Parent
Push-Location -LiteralPath $repo
try {
    $events = & $Cargo test --locked -p diy-ime2-tsf --test stage_a --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'The Win32 test executable could not be built.' }
} finally { Pop-Location }
$executables = @(
    foreach ($event in $events) {
        $item = $event | ConvertFrom-Json
        if ($item.reason -eq 'compiler-artifact' -and $item.target.name -eq 'stage_a' -and $item.executable) {
            $item.executable
        }
    }
)
if ($executables.Count -ne 1) { throw 'Expected exactly one Win32 test executable.' }

$session = Join-Path ([IO.Path]::GetTempPath()) ('diy-ime-tsf-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($session)
$release = Join-Path $session 'release.signal'
$toolHash = (Get-FileHash -LiteralPath $tool).Hash
$dllHash = (Get-FileHash -LiteralPath $dll).Hash
$seconds = if ($StageBHostConfiguration) { (300 + $ManualAppSeconds).ToString() } else { '150' }
$arguments = @(
    '-NoProfile', '-File', ('"' + $machineInstaller + '"'),
    '-BuildDirectory', ('"' + $build + '"'), '-ReleaseFile', ('"' + $release + '"'),
    '-ProfileToolSha256', $toolHash, '-ServiceDllSha256', $dllHash, '-Seconds', $seconds
)
if ($StageBHostConfiguration) {
    $arguments += @(
        '-StageBHostConfiguration', ('"' + $StageBHostConfiguration + '"'),
        '-StageBConfigSha256', (Get-FileHash -LiteralPath $StageBHostConfiguration).Hash,
        '-StageBHostSha256', (Get-FileHash -LiteralPath $stageBHost).Hash
    )
}
$installer = $null
$enableAttempted = $false
try {
    $installer = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $registrationTimeout = 45
    do {
        $status = Get-TestRegistration
        if ($status -match 'registered=true') { break }
        if (Test-Path -LiteralPath $installerStatus -PathType Leaf) {
            $installerError = Get-Content -LiteralPath $installerStatus -ErrorAction SilentlyContinue |
                Where-Object { $_ -like 'error: *' } | Select-Object -Last 1
            if ($installerError) { throw "Temporary registration failed: $installerError" }
        }
        Start-Sleep -Milliseconds 250
    } while (!$installer.HasExited -and $clock.Elapsed.TotalSeconds -lt $registrationTimeout)
    if ($status -notmatch 'registered=true') {
        if ($installer.HasExited) { throw "The registration helper exited before verification (code $($installer.ExitCode))." }
        throw "The machine profile could not be verified within $registrationTimeout seconds."
    }
    $enableAttempted = $true
    & $tool enable
    if ($LASTEXITCODE -ne 0) { throw 'The current user could not enable the dedicated profile.' }
    if ((Get-TestRegistration) -notmatch 'registered=true enabled_for_current_user=true') {
        throw 'The current-user TSF profile did not become enabled.'
    }
    $tests = @('interactive_machine_registered_app_routes_input')
    if ($StageBHostConfiguration) {
        $tests += @('interactive_machine_registered_stage_b','interactive_machine_registered_stage_b_protection')
    }
    $failures = [System.Collections.Generic.List[string]]::new()
    foreach ($test in $tests) {
        & $executables[0] --ignored --exact $test --nocapture
        if ($LASTEXITCODE -ne 0) { $failures.Add($test) }
    }
    if ($ManualAppSeconds -gt 0) {
        if ($installer.HasExited -or $clock.Elapsed.TotalSeconds -ge 285) {
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
                $clock = [Diagnostics.Stopwatch]::StartNew()
                do {
                    $status = Get-TestRegistration
                    $languageAfter = @(Get-LanguageSettings)
                    $clean = ($status -match 'registered=false enabled_for_current_user=false') -and
                        !($languageAfter -match $profile) -and !(Test-DedicatedKeys) -and
                        !(Test-Path -LiteralPath $staging)
                    if (!$clean) { Start-Sleep -Milliseconds 300 }
                } while (!$clean -and $clock.Elapsed.TotalSeconds -lt 8)
                if (!$clean) { throw 'A test registration or staging file remains after cleanup.' }
                if (($languageBaseline -join "`n") -cne ($languageAfter -join "`n")) {
                    throw 'The input-method or default override settings changed from the baseline.'
                }
                'The test profile and staged files are absent. The original input-method settings match.'
            }
        } finally {
            if (!$installer -or $installer.HasExited) {
                [IO.File]::Delete($release)
                [IO.Directory]::Delete($session)
            }
            if ($installer) { $installer.Dispose() }
        }
    }
}
