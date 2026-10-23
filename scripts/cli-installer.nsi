Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
Name "AxiomCLI"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "AxiomCLI"
VIAddVersionKey "FileDescription" "AxiomCLI installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "Astrea Labs, Inc."
VIAddVersionKey "LegalCopyright" "Copyright Astrea Foundation"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\AxiomCLI"
InstallDirRegKey HKCU "Software\AxiomCLI" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!ifdef SIGN_SCRIPT
!uninstfinalize 'pwsh.exe -NoProfile -NonInteractive -File "${SIGN_SCRIPT}" -FilePath "%1"' = 0
!finalize 'pwsh.exe -NoProfile -NonInteractive -File "${SIGN_SCRIPT}" -FilePath "%1"' = 0
!endif
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!macro NativePowerShell
  StrCpy $R3 "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
  IfFileExists "$WINDIR\sysnative\WindowsPowerShell\v1.0\powershell.exe" 0 +2
    StrCpy $R3 "$WINDIR\sysnative\WindowsPowerShell\v1.0\powershell.exe"
!macroend
Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "AxiomCLI requires 64-bit Windows (x64 or ARM64)."
    Abort
  ${EndIf}
FunctionEnd
Section
  SetOutPath "$INSTDIR"
!ifdef INPUT_ARM64
  ${If} ${IsNativeARM64}
    File /r "${INPUT_ARM64}\*"
  ${Else}
    File /r "${INPUT_X64}\*"
  ${EndIf}
!else
  File /r "${INPUT}\*"
!endif
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\AxiomCLI" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AxiomCLI" "DisplayName" "AxiomCLI"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AxiomCLI" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AxiomCLI" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  !insertmacro NativePowerShell
  nsExec::ExecToStack /TIMEOUT=30000 '"$R3" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\windows-cli-path.ps1" -Action Install -BinPath "$INSTDIR\launchers" -Scope User'
  Pop $0
  Pop $1
  ${If} $0 != 0
    SetErrorLevel 1
    Abort "Could not add AxiomCLI to PATH."
  ${EndIf}
  SendMessage 0xffff 0x001A 0 "STR:Environment" /TIMEOUT=5000
SectionEnd
Section "Uninstall"
  !insertmacro NativePowerShell
  nsExec::ExecToStack /TIMEOUT=30000 '"$R3" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\windows-cli-path.ps1" -Action Remove -BinPath "$INSTDIR\launchers" -Scope User'
  Pop $0
  Pop $1
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\launchers"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\axiom-install.json"
  Delete "$INSTDIR\windows-cli-path.ps1"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\AxiomCLI"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AxiomCLI"
  SendMessage 0xffff 0x001A 0 "STR:Environment" /TIMEOUT=5000
SectionEnd
