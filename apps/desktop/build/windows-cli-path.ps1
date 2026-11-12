param(
  [Parameter(Mandatory=$true)][ValidateSet('Install','Remove')][string]$Action,
  [Parameter(Mandatory=$true)][string]$BinPath,
  [ValidateSet('User','Machine')][string]$Scope = 'User'
)
$ErrorActionPreference = 'Stop'
$BinPath = [IO.Path]::GetFullPath($BinPath).TrimEnd('\')
if ($Action -eq 'Install' -and -not (Test-Path -LiteralPath (Join-Path $BinPath 'axiomcli.cmd') -PathType Leaf)) {
  throw 'The bundled axiomcli launcher is missing.'
}
# Read raw registry text: expanding %USERPROFILE% here would corrupt other entries.
$hive = if ($Scope -eq 'Machine') { [Microsoft.Win32.Registry]::LocalMachine } else { [Microsoft.Win32.Registry]::CurrentUser }
$subkey = if ($Scope -eq 'Machine') { 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment' } else { 'Environment' }
$key = $hive.CreateSubKey($subkey)
try {
  $kind = if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
  if ($kind -ne [Microsoft.Win32.RegistryValueKind]::String -and $kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) { throw 'PATH is not a string registry value.' }
  $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
  $parts = @($raw -split ';' | Where-Object { $_.Trim().Trim('"').TrimEnd('\') -ine $BinPath })
  $next = $parts -join ';'
  if ($Action -eq 'Install') {
    # Keep empty/trailing entries too, so uninstall restores the original text.
    $separator = if ($next.Length -gt 0) { ';' } else { '' }
    $next = $next + $separator + $BinPath
  }
  if ($next -ne $raw) { $key.SetValue('Path', $next, $kind) }
} finally { $key.Dispose() }
# The NSIS caller broadcasts WM_SETTINGCHANGE after this script returns.
