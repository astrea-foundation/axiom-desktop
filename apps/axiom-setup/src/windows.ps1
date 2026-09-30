param(
    [Parameter(Mandatory)][ValidateSet('Check','Inspect','Verify','Running','Install')][string] $Action,
    [string] $FilePath,
    [string] $Destination,
    [string] $Publisher,
    [switch] $Machine
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

switch ($Action) {
    'Check' { 'ready' }
    'Inspect' {
        $found = @()
        foreach ($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryHive]::LocalMachine)) {
            foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32)) {
                $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
                try {
                    $key = $base.OpenSubKey('Software\2fdd6a83-7dae-5442-aa32-e7c956c0a096')
                    if ($null -eq $key) { continue }
                    try {
                        $root = $key.GetValue('InstallLocation')
                        if ([string]::IsNullOrWhiteSpace($root)) { continue }
                        $root = [IO.Path]::GetFullPath($root)
                        if (-not (Test-Path -LiteralPath (Join-Path $root 'Axiom.exe'))) { throw 'Registered Axiom installation is incomplete. Repair it with the offline installer.' }
                        $found += @{root=$root;machine=($hive -eq [Microsoft.Win32.RegistryHive]::LocalMachine)}
                    } finally { $key.Dispose() }
                } finally { $base.Dispose() }
            }
        }
        $unique = @($found | Group-Object { "$($_.machine):$($_.root.ToLowerInvariant())" } | ForEach-Object { $_.Group[0] })
        if ($unique.Count -gt 1) { throw 'Multiple Axiom installations were found. Use the update action in the app you want to update.' }
        if ($unique.Count -eq 0) { 'null' } else { $unique[0] | ConvertTo-Json -Compress }
    }
    'Verify' {
        $signature = Get-AuthenticodeSignature -LiteralPath $FilePath
        if ($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate -or $null -eq $signature.TimeStamperCertificate) {
            throw 'The installer has an invalid signature or timestamp.'
        }
        $name = $signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
        if ($name -cne $Publisher) { throw 'The installer was signed by an unexpected publisher.' }
    }
    'Running' {
        # Include other users' sessions, whose per-user leases are separate.
        # An inaccessible process path is busy rather than proof it is safe.
        $root = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($FilePath)).TrimEnd('\') + '\'
        $running = @(Get-Process -Name Axiom,axiomcli,axiom-proxy -ErrorAction SilentlyContinue | Where-Object {
            -not $_.Path -or [IO.Path]::GetFullPath($_.Path).StartsWith($root, [StringComparison]::OrdinalIgnoreCase)
        })
        if ($running.Count -gt 0) { 'true' } else { 'false' }
    }
    'Install' {
        # NSIS interprets /D as the unquoted remainder, and requires it last.
        # Start-Process joins the array without shell evaluation.
        $scope = if ($Machine) { '/allusers' } else { '/currentuser' }
        $arguments = @('/S', $scope, "/D=$Destination")
        if ($Machine) {
            $process = Start-Process -FilePath $FilePath -ArgumentList $arguments -Verb RunAs -PassThru
        } else {
            $process = Start-Process -FilePath $FilePath -ArgumentList $arguments -PassThru
        }
        $null = $process.Handle
        # Keep the lease and verified staging alive until NSIS actually exits.
        # A timeout must not offer another installer while the first is running.
        $process.WaitForExit()
        $process.Refresh()
        if ($process.ExitCode -ne 0) { throw "The installer did not finish successfully (exit $($process.ExitCode))." }
    }
}
