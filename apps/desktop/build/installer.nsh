Var axiomFreshInstall

!macro customInit
  ; Inspect installation ownership before NSIS writes its new registry entries.
  ; Existing Desktop installations, including versions predating this feature,
  ; must not unexpectedly start installing updates on their next launch.
  StrCpy $axiomFreshInstall "1"
  ReadRegStr $R0 HKCU "${INSTALL_REGISTRY_KEY}" "InstallLocation"
  ReadRegStr $R1 HKLM "${INSTALL_REGISTRY_KEY}" "InstallLocation"
  ${if} $R0 != ""
    StrCpy $axiomFreshInstall "0"
  ${endif}
  ${if} $R1 != ""
    StrCpy $axiomFreshInstall "0"
  ${endif}
!macroend

!macro axiomNativePowerShell
  ; NSIS runs as x86. Use native PowerShell on x64/ARM64 Windows instead of
  ; the emulated SysWOW64 host.
  StrCpy $R3 "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
  IfFileExists "$WINDIR\sysnative\WindowsPowerShell\v1.0\powershell.exe" 0 +2
    StrCpy $R3 "$WINDIR\sysnative\WindowsPowerShell\v1.0\powershell.exe"
!macroend

!macro customInstall
  ${if} $axiomFreshInstall == "1"
    FileOpen $R0 "$INSTDIR\resources\axiom-first-launch.json" w
    FileWrite $R0 '{$\"schemaVersion$\":1}'
    FileClose $R0
  ${endif}
  StrCpy $R0 "User"
  ${if} $installMode == "all"
    StrCpy $R0 "Machine"
  ${endIf}
  !insertmacro axiomNativePowerShell
  nsExec::ExecToStack /TIMEOUT=30000 '"$R3" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\resources\installer\windows-cli-path.ps1" -Action Install -BinPath "$INSTDIR\resources\launchers" -Scope $R0'
  Pop $R1
  Pop $R2
  SendMessage 0xffff 0x001A 0 "STR:Environment" /TIMEOUT=5000
  ${if} $R1 != "0"
    MessageBox MB_OK|MB_ICONEXCLAMATION "Axiom was installed, but the CLI could not be added to PATH. Run axiomcli.cmd from $INSTDIR\resources\launchers, or add that directory to PATH." /SD IDOK
  ${endIf}
!macroend

!macro customUnInstall
  StrCpy $R0 "User"
  ${if} $installMode == "all"
    StrCpy $R0 "Machine"
  ${endIf}
  IfFileExists "$INSTDIR\resources\installer\windows-cli-path.ps1" 0 axiom_cli_path_done
  !insertmacro axiomNativePowerShell
  nsExec::ExecToStack /TIMEOUT=30000 '"$R3" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\resources\installer\windows-cli-path.ps1" -Action Remove -BinPath "$INSTDIR\resources\launchers" -Scope $R0'
  Pop $R1
  Pop $R2
  SendMessage 0xffff 0x001A 0 "STR:Environment" /TIMEOUT=5000
  axiom_cli_path_done:
!macroend
