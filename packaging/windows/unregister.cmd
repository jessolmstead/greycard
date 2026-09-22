@echo off
rem Take back what register.cmd wrote. Nothing here is outside
rem HKCU\Software\Classes, so there is no elevation prompt.
rem
rem     unregister.cmd
rem
rem The OpenWithProgids entries go one value at a time rather than by
rem deleting the key: another application's offer for .cr3 lives in the
rem same key, and dropping the key would take that with it.
setlocal

set "classes=HKCU\Software\Classes"
echo unregistering greycard for %USERNAME%

for %%K in (greycard.gcd greycard.photo Applications\greycard-ui.exe) do (
    reg delete "%classes%\%%K" /f >nul 2>&1
)

rem The extension is only ours to drop if it still points at our type;
rem if something else has taken .gcd since, leave its choice alone.
for /f "tokens=2,*" %%A in ('reg query "%classes%\.gcd" /ve 2^>nul ^| findstr /r "REG_SZ"') do (
    if /i "%%B"=="greycard.gcd" reg delete "%classes%\.gcd" /f >nul 2>&1
)
reg delete "%classes%\.gcd\OpenWithProgids" /v "greycard.gcd" /f >nul 2>&1

for %%E in (.cr3 .cr2 .dng .nef .arw .raf .orf .rw2) do (
    reg delete "%classes%\%%E\OpenWithProgids" /v "greycard.photo" /f >nul 2>&1
)

powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-Type -Namespace G -Name S -MemberDefinition '[DllImport(\"shell32.dll\")] public static extern void SHChangeNotify(int e, uint f, IntPtr a, IntPtr b);'; [G.S]::SHChangeNotify(0x8000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)" >nul 2>&1

echo done.
