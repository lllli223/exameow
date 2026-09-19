@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0exameowctl.ps1" %*
exit /b %ERRORLEVEL%
