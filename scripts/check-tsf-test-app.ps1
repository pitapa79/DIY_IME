param(
    [Parameter(Mandatory)][string]$StagingDirectory,
    [int[]]$AppProcessId
)
$expected = [IO.Path]::GetFullPath((Join-Path $StagingDirectory 'diy_ime2_tsf.dll'))
$apps = if ($AppProcessId) {
    Get-Process -Id $AppProcessId -ErrorAction Stop
} else {
    Get-Process | Where-Object MainWindowHandle -ne 0
}
foreach ($app in $apps) {
    try {
        $modules = $app.Modules
        if (!$modules) { throw 'Modules inaccessible' }
        $loaded = @($modules | Where-Object ModuleName -eq 'diy_ime2_tsf.dll')
    } catch {
        if ($AppProcessId) { Write-Warning "Cannot inspect $($app.ProcessName) (PID $($app.Id))." }
        continue
    }
    if (!$loaded -and $AppProcessId) {
        Write-Warning "Test DLL not observed in $($app.ProcessName) (PID $($app.Id))."
    }
    foreach ($module in $loaded) {
        if ($module.FileName -ne $expected) {
            Write-Warning "$($app.ProcessName) (PID $($app.Id)) loaded another staging directory. Restart this app before evaluating the current build."
        } else {
            "Current test DLL loaded in $($app.ProcessName) (PID $($app.Id))."
        }
    }
}
