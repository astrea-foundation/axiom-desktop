param([Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string] $Arch)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$desktop = Split-Path $PSScriptRoot -Parent
Push-Location $desktop
try {
    $version = (Get-Content package.json -Raw | ConvertFrom-Json).version
    $folder = if ($Arch -eq 'arm64') { 'win-arm64-unpacked' } else { 'win-unpacked' }
    $unpacked = (Resolve-Path (Join-Path 'dist' $folder)).Path
    & "$PSScriptRoot/verify-windows-package.ps1" -Unpacked $unpacked -Arch $Arch -Version $version
    # --prepackaged skips electron-builder's app signing hooks. Sign the entire
    # executable payload explicitly BEFORE NSIS compresses it, including the CLI.
    $native = @(Get-ChildItem -LiteralPath $unpacked -Recurse -File | Where-Object { $_.Extension -in '.exe', '.dll', '.node', '.ps1' })
    foreach ($file in $native) { & "$PSScriptRoot/sign-windows.ps1" -FilePath $file.FullName }
    # afterPack doesn't run for --prepackaged: retain our NSIS-compatible filter.
    $env:ELECTRON_BUILDER_7Z_FILTER = 'BCJ'
    foreach ($name in @('CSC_LINK', 'CSC_KEY_PASSWORD', 'WIN_CSC_LINK', 'WIN_CSC_KEY_PASSWORD')) {
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
    & pnpm exec electron-builder --win nsis "--$Arch" --prepackaged $unpacked --config electron-builder.windows-signing.cjs --publish never
    if ($LASTEXITCODE -ne 0) { throw 'Signed NSIS packaging failed' }
    $installer = Join-Path 'dist' "Axiom-$version-win-$Arch.exe"
    & "$PSScriptRoot/verify-windows-package.ps1" -Unpacked $unpacked -Arch $Arch -Version $version -Installer $installer -Publisher $env:AXIOM_SIGNING_PUBLISHER
} finally { Pop-Location }
