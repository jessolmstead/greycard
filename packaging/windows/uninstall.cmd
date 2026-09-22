@echo off
rem Take greycard back out. Double-click it, or:
rem
rem     uninstall.cmd
rem
rem Settings, presets and the cached models are left alone; they are
rem under %APPDATA%\greycard and %LOCALAPPDATA%\greycard and are not
rem this script's to throw away. See install.cmd for why it waits for
rem a key.
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Remove
set "code=%ERRORLEVEL%"
echo.
pause
exit /b %code%
