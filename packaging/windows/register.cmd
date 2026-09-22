@echo off
rem Tell Explorer about greycard, for this user only.
rem
rem     register.cmd
rem
rem Everything goes under HKCU\Software\Classes, so there is no
rem elevation prompt and nothing is written for other accounts.
rem unregister.cmd takes it all back out. install.cmd runs this for
rem you; it is here on its own for the copy you unpacked and kept
rem where it landed.
rem
rem Two document types, the pair notes section 120 settled on:
rem
rem   greycard.gcd   the edit sidecar. It gets the extension outright,
rem                  since nothing else claims .gcd, and its own icon:
rem                  two of the app tile's cards on a document, so a
rem                  folder of frames does not read as a folder of
rem                  application tiles.
rem   greycard.photo the frames themselves. This one is offered, not
rem                  taken: it joins each raw extension's
rem                  OpenWithProgids, which puts greycard in the Open
rem                  with list and leaves whatever already opens a
rem                  .cr3 alone. Someone who wants it as the default
rem                  picks it there once, and Windows records that
rem                  choice where a script is not allowed to.
rem
rem The app's own icon is read out of greycard-ui.exe, which carries it
rem as a resource; only the sidecar needs an .ico on disk beside it.
setlocal

rem Run from the unpacked archive the binaries are in bin\; run from an
rem installed copy they are beside this script.
set "app="
if exist "%~dp0bin\greycard-ui.exe" set "app=%~dp0bin\greycard-ui.exe"
if not defined app if exist "%~dp0greycard-ui.exe" set "app=%~dp0greycard-ui.exe"
if not defined app (
    echo register.cmd: no greycard-ui.exe beside this script or in bin\ below it.
    exit /b 1
)
for %%I in ("%app%") do set "dir=%%~dpI"
set "sidecar_icon=%dir%application-x-greycard-edit.ico"

echo registering greycard for %USERNAME%
echo   %app%

rem The application, so "Open with" shows a name rather than the file.
set "classes=HKCU\Software\Classes"
reg add "%classes%\Applications\greycard-ui.exe" /v FriendlyAppName /d "greycard" /f >nul
reg add "%classes%\Applications\greycard-ui.exe\shell\open\command" /ve /d "\"%app%\" \"%%1\"" /f >nul

rem The sidecar: its own type, its own icon, and the extension.
reg add "%classes%\greycard.gcd" /ve /d "greycard edit sidecar" /f >nul
reg add "%classes%\greycard.gcd\shell\open\command" /ve /d "\"%app%\" \"%%1\"" /f >nul
if exist "%sidecar_icon%" (
    reg add "%classes%\greycard.gcd\DefaultIcon" /ve /d "\"%sidecar_icon%\"" /f >nul
) else (
    echo   note: %sidecar_icon% is missing, so sidecars keep the generic icon
)
reg add "%classes%\.gcd" /ve /d "greycard.gcd" /f >nul
reg add "%classes%\.gcd\OpenWithProgids" /v "greycard.gcd" /t REG_NONE /f >nul
echo   .gcd

rem The frames: offered on each raw extension, never taken. The list is
rem the browser's, crates/greycard-ui/src/files.rs.
reg add "%classes%\greycard.photo" /ve /d "greycard photo" /f >nul
reg add "%classes%\greycard.photo\DefaultIcon" /ve /d "\"%app%\",0" /f >nul
reg add "%classes%\greycard.photo\shell\open\command" /ve /d "\"%app%\" \"%%1\"" /f >nul
for %%E in (.cr3 .cr2 .dng .nef .arw .raf .orf .rw2) do (
    reg add "%classes%\%%E\OpenWithProgids" /v "greycard.photo" /t REG_NONE /f >nul
    echo   %%E ^(offered^)
)

rem Explorer caches associations; this is the call that tells it not to.
rem It is a convenience, so a failure here is not one: the entries are
rem on disk either way and a sign-out picks them up.
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-Type -Namespace G -Name S -MemberDefinition '[DllImport(\"shell32.dll\")] public static extern void SHChangeNotify(int e, uint f, IntPtr a, IntPtr b);'; [G.S]::SHChangeNotify(0x8000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)" >nul 2>&1

echo done. unregister.cmd undoes this.
