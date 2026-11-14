; The Windows installer, for Inno Setup 7. scripts/windows-setup.ps1
; compiles it from the directory scripts/package.sh staged, so the
; setup and the zip carry the same files:
;
;     scripts/windows-setup.ps1
;
; It installs what install.ps1 installs, the same way: for this user
; only, with no elevation, into %LOCALAPPDATA%\Programs\greycard (one
; directory, the binaries and their libraries together), a Start menu
; entry, the directory on the user's Path, and the file associations
; register.cmd writes, by running register.cmd. Uninstalling, from
; Settings > Apps > Installed apps, takes all of it back out, the
; associations by unregister.cmd, and leaves the settings, presets,
; and cached models under %APPDATA%\greycard and %LOCALAPPDATA%\greycard
; alone, as uninstall.cmd does.
;
; Defined on the compiler's command line, never here, so a version is
; written once (Cargo.toml, which the tag must match):
;
;   Version         the workspace version, 0.12.0 or 0.12.0-rc.1
;   NumericVersion  its first three numbers, for the file's version
;                   resource, which takes digits alone
;   Stage           the absolute path of package.sh's staging directory
;   Arch            x64compatible or arm64, the Windows the binaries
;                   are for
;
; A sign tool named greycard, when the compiler is given one, signs
; the setup and its uninstaller (scripts/windows-setup.ps1 passes it
; when WINDOWS_SIGN_SCRIPT is set; see the release workflow).

#ifndef Version
  #error Version is not defined; compile through scripts/windows-setup.ps1
#endif
#ifndef NumericVersion
  #error NumericVersion is not defined; compile through scripts/windows-setup.ps1
#endif
#ifndef Stage
  #error Stage is not defined; compile through scripts/windows-setup.ps1
#endif
#ifndef Arch
  #error Arch is not defined; compile through scripts/windows-setup.ps1
#endif

[Setup]
; The key the uninstall entry is filed under. Never change it: a new
; one makes the next version a second install beside the first.
AppId=greycard
AppName=greycard
AppVersion={#Version}
AppVerName=greycard {#Version}
AppPublisher=Jess Olmstead
AppPublisherURL=https://greycard.org
AppSupportURL=https://github.com/jessolmstead/greycard/issues
AppUpdatesURL=https://github.com/jessolmstead/greycard/releases
VersionInfoVersion={#NumericVersion}

; Per user, never elevated, as install.ps1. The auto constants resolve
; to the user's own folders here ({autopf} is %LOCALAPPDATA%\Programs),
; so an all-users mode later is PrivilegesRequiredOverridesAllowed
; and the HKLM side of the Path, not a rewrite.
PrivilegesRequired=lowest
DefaultDirName={autopf}\greycard
; install.ps1 asks nothing; /DIR= on the setup's command line is the
; installer's GREYCARD_PREFIX.
DisableDirPage=yes
DisableProgramGroupPage=yes

ArchitecturesAllowed={#Arch}
ArchitecturesInstallIn64BitMode={#Arch}
MinVersion=10.0

; Explorer is told about the Path and the associations at the end of
; install and of uninstall, as install.ps1 and register.cmd tell it.
ChangesEnvironment=yes
ChangesAssociations=yes

SetupIconFile={#AddBackslash(SourcePath)}..\..\assets\icon\greycard.ico
UninstallDisplayIcon={app}\greycard-ui.exe
UninstallDisplayName=greycard
WizardStyle=modern dynamic
Compression=lzma2/max
SolidCompression=yes

#ifdef Sign
SignTool=greycard
#endif

[Files]
; bin\ lands as the install directory itself, as install.ps1 copies
; it, and the five files install.ps1 copies beside it. ignoreversion:
; the script overwrites whatever is there, and so does this.
Source: "{#Stage}\bin\*"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY-NOTICES"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\register.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\unregister.cmd"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; greycard.lnk in the Start menu's Programs folder, no group, as
; install.ps1 makes it. The icon is the executable's own.
Name: "{autoprograms}\greycard"; Filename: "{app}\greycard-ui.exe"; WorkingDir: "{app}"; Comment: "A raw editor that gets the light right"

[Run]
; The associations are register.cmd's, run as install.ps1 runs it, so
; the list of raw extensions stays in the two scripts and the browser.
; Doubled quotes: cmd /c strips the outer pair, which keeps the inner
; one whatever characters the user's profile path holds.
Filename: "{cmd}"; Parameters: "/c """"{app}\register.cmd"""""; Flags: runhidden
Filename: "{app}\greycard-ui.exe"; Description: "{cm:LaunchProgram,greycard}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Before the files go, so unregister.cmd is still there to run.
Filename: "{cmd}"; Parameters: "/c """"{app}\unregister.cmd"""""; Flags: runhidden; RunOnceId: "unregister"

[Code]
// The user's Path, as install.ps1 keeps it: read as stored, with its
// %VARIABLES% unexpanded (RegQueryStringValue does not expand), the
// install directory appended once, and taken back out on uninstall,
// compared without case or a trailing backslash. Written back as
// REG_EXPAND_SZ, which is what Windows makes a user Path; install.ps1
// keeps whichever kind it found, and Pascal Script cannot read a
// value's kind.

function SamePath(const A, B: String): Boolean;
begin
  Result := SameText(RemoveBackslash(A), RemoveBackslash(B));
end;

procedure AddToPath;
var
  Value, Dir: String;
  Entries: TArrayOfString;
  I, N: Integer;
begin
  Dir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Value) then begin
    // A Path of some other type is left alone: never write one we
    // could not read.
    if RegValueExists(HKCU, 'Environment', 'Path') then begin
      Log('The user Path is not a string; greycard is not put on it.');
      Exit;
    end;
    Value := '';
  end;
  Entries := StringSplit(Value, [';'], stExcludeEmpty);
  N := GetArrayLength(Entries);
  for I := 0 to N - 1 do
    if SamePath(Entries[I], Dir) then
      Exit;
  SetArrayLength(Entries, N + 1);
  Entries[N] := Dir;
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', StringJoin(';', Entries));
end;

procedure RemoveFromPath;
var
  Value, Dir: String;
  Entries, Kept: TArrayOfString;
  I, N: Integer;
begin
  Dir := ExpandConstant('{app}');
  // Absent or not a string: nothing of ours to take off.
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Value) then
    Exit;
  Entries := StringSplit(Value, [';'], stExcludeEmpty);
  N := 0;
  for I := 0 to GetArrayLength(Entries) - 1 do
    if not SamePath(Entries[I], Dir) then begin
      SetArrayLength(Kept, N + 1);
      Kept[N] := Entries[I];
      N := N + 1;
    end;
  if N <> GetArrayLength(Entries) then
    RegWriteExpandStringValue(HKCU, 'Environment', 'Path', StringJoin(';', Kept));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddToPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveFromPath;
end;
