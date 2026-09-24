@echo off
rem Take greycard back out. Double-click it, or:
rem
rem     uninstall.cmd
rem
rem Settings, presets, the cached models and the thumbnail cache are
rem left alone; they are under %APPDATA%\greycard and
rem %LOCALAPPDATA%\greycard (the thumbnails in its thumbs folder, which
rem can be deleted at any time) and are not this script's to throw
rem away. See install.cmd for why it waits for a key.
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Remove
set "code=%ERRORLEVEL%"
echo.
pause
exit /b %code%
