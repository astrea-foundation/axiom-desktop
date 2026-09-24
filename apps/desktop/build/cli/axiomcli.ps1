$ErrorActionPreference = 'Stop'
# Windows PowerShell 5's native invocation loses embedded quotes. Pass the
# Windows argv encoding directly, preserving empty arguments and backslashes.
function Invoke-AxiomNative([string]$File, [string[]]$Values) {
    $quoted = foreach ($value in $Values) {
        $escaped = [regex]::Replace($value, '(\\*)"', '$1$1\"')
        $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
        '"' + $escaped + '"'
    }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $File
    $start.Arguments = $quoted -join ' '
    $start.UseShellExecute = $false
    $start.WorkingDirectory = (Get-Location).Path
    $child = [Diagnostics.Process]::Start($start)
    try { $child.WaitForExit(); return $child.ExitCode } finally { $child.Dispose() }
}
$runtime = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../bin/axiomcli.exe'))
$arguments = @($args)
$directory = Join-Path ([IO.Path]::GetTempPath()) ('axiom-console-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($directory) | Out-Null
$oldHandoff = $env:AXIOM_UPDATE_HANDOFF
$env:AXIOM_UPDATE_HANDOFF = Join-Path $directory 'handoff'
try {
    while ($true) {
        $result = Invoke-AxiomNative $runtime $arguments
        if ($result -ne 85) { exit $result }
        $jobPath = (Get-Content -LiteralPath $env:AXIOM_UPDATE_HANDOFF -Raw).Trim()
        Remove-Item -LiteralPath $env:AXIOM_UPDATE_HANDOFF
        $job = Get-Content -LiteralPath $jobPath -Raw | ConvertFrom-Json
        $helper = Join-Path (Split-Path $jobPath -Parent) 'update-helper.exe'
        $result = Invoke-AxiomNative $helper @('update','--apply-job',$jobPath)
        if ($result -ne 0) { exit $result }
        # No shell evaluation: the native installer revalidates the job and its
        # signed manifest. The launcher keeps this console alive during replacement.
        $arguments = @('tui', '--cwd', [string]$job.restart.cwd)
        if ($job.restart.resume) { $arguments += @('--resume', [string]$job.restart.resume) }
    }
} finally {
    $env:AXIOM_UPDATE_HANDOFF = $oldHandoff
    Remove-Item -LiteralPath $directory -Recurse -Force -ErrorAction SilentlyContinue
}
