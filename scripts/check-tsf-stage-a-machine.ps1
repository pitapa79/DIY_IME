param(
    [Parameter(Mandatory)][string]$BuildDirectory,
    [string]$Cargo = 'cargo',
    [string]$StageBHostConfiguration,
    [switch]$CheckPrerequisites
)
$ErrorActionPreference = 'Stop'
if (![Environment]::Is64BitProcess -or ![Environment]::UserInteractive) {
    throw 'Run on an interactive x64 Windows desktop.'
}

$service = '{EECF857D-82EB-4C57-9592-257F6ECF62F1}'
$profile = '8c73cb26-1ccf-45e1-a259-f29b674219e0'
$build = (Resolve-Path -LiteralPath $BuildDirectory).Path
$tool = Join-Path $build 'diy-ime2-tsf-test-profile.exe'
$dll = Join-Path $build 'diy_ime2_tsf.dll'
$staging = Join-Path $env:ProgramFiles "DIY-IME-Stage-A-$service"
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
$arguments = @(
    '-NoProfile', '-File', ('"' + $machineInstaller + '"'),
    '-BuildDirectory', ('"' + $build + '"'), '-ReleaseFile', ('"' + $release + '"'),
    '-ProfileToolSha256', $toolHash, '-ServiceDllSha256', $dllHash, '-Seconds', '150'
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
    do {
        $status = Get-TestRegistration
        if ($status -match 'registered=true') { break }
        Start-Sleep -Milliseconds 250
    } while (!$installer.HasExited -and $clock.Elapsed.TotalSeconds -lt 15)
    if ($status -notmatch 'registered=true') { throw 'The machine profile could not be verified.' }
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
    foreach ($test in $tests) {
        & $executables[0] --ignored --exact $test --nocapture
        if ($LASTEXITCODE -ne 0) { throw "The Win32 TSF test failed: $test" }
    }
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
