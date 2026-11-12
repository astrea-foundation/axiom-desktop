@echo off
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0axiomcli.ps1" %*
exit /b %errorlevel%
