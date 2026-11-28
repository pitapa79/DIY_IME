function Get-TsfStagingDirectory([guid]$RunId, [bool]$FreshStaging) {
    $name = 'DIY-IME-Stage-A-{EECF857D-82EB-4C57-9592-257F6ECF62F1}'
    if ($FreshStaging) { $name += '-' + $RunId.ToString('N') }
    return Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $name
}

function Assert-ReusableTsfStaging([string]$Directory, [hashtable]$Hashes) {
    $folder = Get-Item -LiteralPath $Directory -Force
    if (!$folder.PSIsContainer -or ($folder.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'The previous TSF staging path is not an ordinary directory.'
    }

    $files = @(Get-ChildItem -LiteralPath $Directory -Force)
    if ($files.Count -ne ($Hashes.Count + 1)) {
        throw 'The previous TSF staging directory has an unexpected file count.'
    }
    foreach ($file in $files) {
        if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
            ($file.Name -ne 'test-status.txt' -and !$Hashes.ContainsKey($file.Name))) {
            throw 'The previous TSF staging directory has unexpected contents.'
        }
        if ($file.Name -ne 'test-status.txt' -and
            (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash -ne $Hashes[$file.Name]) {
            throw "The previous staged file does not match this build: $($file.Name)"
        }
    }
    $report = @(Get-Content -LiteralPath (Join-Path $Directory 'test-status.txt'))
    if ($report -notcontains 'registration removed' -or $report[-1] -notlike 'cleanup error: *') {
        throw 'The previous TSF staging directory is missing the unregistration and cleanup record.'
    }
}

function Invoke-TsfProfileCleanup([string]$Tool, [ValidateSet('disable','remove')][string]$Action, [int]$TimeoutMs = 5000) {
    $process = Start-Process -FilePath $Tool -ArgumentList $Action -WindowStyle Hidden -PassThru
    try {
        if (!$process.WaitForExit($TimeoutMs)) {
            try { $process.Kill() } catch [InvalidOperationException] { if (!$process.HasExited) { throw } }
            if (!$process.WaitForExit(2000)) { throw "The TSF profile tool did not exit after cancellation ($Action)." }
            throw "The TSF profile tool timed out ($Action)."
        }
        if ($process.ExitCode -ne 0) { throw "The TSF profile tool failed ($Action, exit code $($process.ExitCode))." }
    } finally { $process.Dispose() }
}
