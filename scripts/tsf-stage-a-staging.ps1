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
