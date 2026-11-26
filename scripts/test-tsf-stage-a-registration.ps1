param(
    [Parameter(Mandatory)][string]$BuildDirectory,
    [Parameter(Mandatory)][string]$ReleaseFile,
    [Parameter(Mandatory)][ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$ProfileToolSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$ServiceDllSha256,
    [string]$StageBHostConfiguration,
    [ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$StageBConfigSha256,
    [ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$StageBHostSha256,
    [ValidateRange(30, 600)][int]$Seconds = 180
)
$ErrorActionPreference = 'Stop'
$service = '{EECF857D-82EB-4C57-9592-257F6ECF62F1}'
$classRoot = 'Software\Classes\CLSID'
$tsfRoot = 'Software\Microsoft\CTF\TIP'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or ![Environment]::Is64BitProcess) {
    throw 'An elevated x64 Windows process is required for the test machine registration.'
}

$release = [IO.Path]::GetFullPath($ReleaseFile)
$build = [IO.Path]::GetFullPath($BuildDirectory)
$machine = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
$directory = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) "DIY-IME-Stage-A-$service"
$dllName = 'diy_ime2_tsf.dll'
$toolName = 'diy-ime2-tsf-test-profile.exe'
$dll = Join-Path $directory $dllName
$tool = Join-Path $directory $toolName
$stamp = Join-Path $directory 'test-status.txt'
$hostName = 'diy-ime2-external-host.exe'
$hostFile = Join-Path $directory $hostName
$hostConfiguration = Join-Path $directory 'ime-execution.json'
if ($StageBHostConfiguration -and (!$StageBConfigSha256 -or !$StageBHostSha256)) {
    throw 'The Stage B files require exact checksums.'
}

function Test-RegistryKey([string]$Path) {
    $key = $machine.OpenSubKey($Path)
    if ($null -eq $key) { return $false }
    $key.Dispose()
    return $true
}

function Write-Status([string]$Text) {
    Write-Host $Text
    if ($ownsFiles) { [IO.File]::AppendAllText($stamp, $Text + [Environment]::NewLine) }
}

$ownsFiles = $false
$ownsRegistration = $false
try {
    Write-Status 'started'
    if ((Test-Path -LiteralPath $directory) -or
        (Test-RegistryKey "$classRoot\$service") -or (Test-RegistryKey "$tsfRoot\$service")) {
        throw 'The dedicated test directory or service already exists.'
    }

    New-Item -ItemType Directory -Path $directory | Out-Null
    $ownsFiles = $true
    [IO.File]::Copy((Join-Path $build $dllName), $dll)
    [IO.File]::Copy((Join-Path $build $toolName), $tool)
    if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -ne $ServiceDllSha256 -or
        (Get-FileHash -LiteralPath $tool -Algorithm SHA256).Hash -ne $ProfileToolSha256) {
        throw 'The staged verification files do not match their source checksums.'
    }

    if ($StageBHostConfiguration) {
        [IO.File]::Copy((Join-Path $build $hostName), $hostFile)
        [IO.File]::Copy([IO.Path]::GetFullPath($StageBHostConfiguration), $hostConfiguration)
        if ((Get-FileHash -LiteralPath $hostFile -Algorithm SHA256).Hash -ne $StageBHostSha256 -or
            (Get-FileHash -LiteralPath $hostConfiguration -Algorithm SHA256).Hash -ne $StageBConfigSha256) {
            throw 'The staged Stage B files do not match their source checksums.'
        }
    }

    $classes = $machine.OpenSubKey($classRoot, $true)
    try {
        $ownedClass = $classes.CreateSubKey($service)
        $ownsRegistration = $true
        try {
            $server = $ownedClass.CreateSubKey('InprocServer32')
            try {
                $server.SetValue('', $dll, 'String')
                $server.SetValue('ThreadingModel', 'Apartment', 'String')
            } finally { $server.Dispose() }
        } finally { $ownedClass.Dispose() }
    } finally { $classes.Dispose() }

    $registerOutput = & $tool register 2>&1
    if ($LASTEXITCODE -ne 0) { throw ($registerOutput -join ' ') }
    Write-Status 'installed'
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $Seconds -and !(Test-Path -LiteralPath $release)) {
        Start-Sleep -Milliseconds 300
    }
} catch {
    Write-Status "error: $($_.Exception.Message)"
    throw
} finally {
    try {
        if ($ownsRegistration) {
            $removeOutput = & $tool remove 2>&1
            if ($LASTEXITCODE -ne 0) { Write-Status "TSF API cleanup: $($removeOutput -join ' ')" }
            foreach ($root in $classRoot, $tsfRoot) {
                if (!(Test-RegistryKey "$root\$service")) { continue }
                $container = $machine.OpenSubKey($root, $true)
                try { $container.DeleteSubKeyTree($service, $false) } finally { $container.Dispose() }
            }
            if ((Test-RegistryKey "$classRoot\$service") -or (Test-RegistryKey "$tsfRoot\$service")) {
                throw 'The temporary registration is still present.'
            }
            Write-Status 'registration removed'
        }
        if ($ownsFiles) {
            foreach ($path in $dll, $tool, $hostFile, $hostConfiguration) {
                if (Test-Path -LiteralPath $path) { [IO.File]::Delete($path) }
            }
            [IO.File]::Delete($stamp)
            [IO.Directory]::Delete($directory)
            $ownsFiles = $false
            Write-Status 'staging removed'
        }
    } catch {
        Write-Status "cleanup error: $($_.Exception.Message)"
        throw
    } finally { $machine.Dispose() }
}
