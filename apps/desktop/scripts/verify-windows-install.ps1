param(
    [Parameter(Mandatory)][string] $Installers,
    [Parameter(Mandatory)][string] $Version,
    [Parameter(Mandatory)][ValidateSet('x64','arm64')][string] $Arch,
    [string] $Publisher
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -eq 'true' -and -not $Publisher) { throw 'Production installation verification requires the expected publisher' }
$Installers = (Resolve-Path $Installers).Path
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('axiom-install-check-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
. "$PSScriptRoot/windows-signature.ps1"

function Invoke-Installer([string] $file, [string[]] $arguments) {
    $process = Start-Process -FilePath $file -ArgumentList $arguments -PassThru
    $null = $process.Handle
    if (-not $process.WaitForExit(180000)) { throw "Installer timed out: $file" }
    $process.Refresh()
    if ($process.ExitCode -ne 0) { throw "Installer failed ($($process.ExitCode)): $file" }
}
function Assert-Cli([string] $file) {
    $reported = & $file --version
    if ($LASTEXITCODE -ne 0 -or $reported -cne "axiomcli $Version") { throw 'Installed CLI version mismatch' }
    $keys = & $file update --trust-keys
    if ($LASTEXITCODE -ne 0 -or -not $env:AXIOM_UPDATE_PUBLIC_KEYS -or $keys -cne $env:AXIOM_UPDATE_PUBLIC_KEYS) { throw 'Installed update trust mismatch' }
}

foreach ($product in @('Axiom','AxiomCLI')) {
    $destination = Join-Path $temporary $product
    $installer = Join-Path $Installers "$product-$Version-win-universal.exe"
    if ($Publisher) { Assert-AxiomSignature -FilePath $installer -Publisher $Publisher }
    # NSIS requires /D last and unquoted, even when the destination has spaces.
    Invoke-Installer $installer @('/S',"/D=$destination")
    if ($product -eq 'Axiom') {
        & "$PSScriptRoot/verify-windows-package.ps1" -Unpacked $destination -Arch $Arch -Version $Version -Publisher $Publisher
        Assert-Cli (Join-Path $destination 'resources/bin/axiomcli.exe')
        $uninstaller = Join-Path $destination 'Uninstall Axiom.exe'
    } else {
        foreach ($name in @('axiomcli.exe','axiom-proxy.exe')) {
            $binary = Join-Path $destination "bin/$name"
            & node -e "require(process.argv[1])(process.argv[2], 'win', process.argv[3])" "$PSScriptRoot/../../../scripts/native-binary.cjs" $binary $Arch
            if ($LASTEXITCODE -ne 0) { throw 'Installer selected the wrong native payload' }
            if ($Publisher) { Assert-AxiomSignature -FilePath $binary -Publisher $Publisher }
        }
        Assert-Cli (Join-Path $destination 'bin/axiomcli.exe')
        $uninstaller = Join-Path $destination 'Uninstall.exe'
        $registered = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\AxiomCLI').UninstallString
        if ($registered -cne "`"$uninstaller`"") { throw 'Registered CLI uninstall command is not correctly quoted' }
    }
    # _?= runs the uninstaller in place so completion is observable by this process.
    Invoke-Installer $uninstaller @('/S',"_?=$destination")
    if (Test-Path (Join-Path $destination 'bin/axiomcli.exe')) { throw 'CLI uninstall left its executable behind' }
    if (Test-Path (Join-Path $destination 'Axiom.exe')) { throw 'Desktop uninstall left its executable behind' }
    Write-Host "PASS installed native $product $Version on $Arch and uninstalled it"
}
# The ARM64 emulation host can briefly retain the uninstaller's image mapping
# after process exit. Keep cleanup bounded without weakening installation checks.
for ($attempt = 0; $attempt -lt 40; $attempt++) {
    if (-not (Test-Path -LiteralPath $temporary)) { break }
    try {
        Remove-Item -LiteralPath $temporary -Recurse -Force
        break
    } catch {
        if ($attempt -eq 39) {
            Write-Warning "Verified installers; temporary cleanup remains locked: $($_.Exception.Message)"
            break
        }
        Start-Sleep -Milliseconds 250
    }
}
