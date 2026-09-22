@echo off
rem Install greycard for this user. Double-click it, or:
rem
rem     install.cmd
rem
rem The work is in install.ps1 beside this; this file is the part
rem Explorer will run on a double-click, which a .ps1 is not.
rem -ExecutionPolicy Bypass applies to this one run and changes no
rem setting: the default policy refuses an unsigned script, and the
rem script it is refusing came out of the archive with this file.
rem
rem It waits for a key at the end, because double-clicked this window
rem closes with the script and takes whatever it said with it. Run
rem install.ps1 directly for the version that does not wait.
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
set "code=%ERRORLEVEL%"
echo.
pause
exit /b %code%
