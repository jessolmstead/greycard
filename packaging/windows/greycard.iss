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
; register.cmd writes, written here as [Registry] entries.
; Uninstalling, from Settings > Apps > Installed apps, takes all of it
; back out and leaves the settings, presets, and cached models under
; %APPDATA%\greycard and %LOCALAPPDATA%\greycard alone, as uninstall.cmd
; does.
;
; Setup starts no program of its own: no cmd, reg, or PowerShell. An
; unsigned setup that runs a hidden shell is what an antivirus's
; heuristics are written to catch, and Inno does all of it itself.
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
; And for a package signed through SignPath alone:
;
;   SignedUninstallerDir  the absolute path of the directory the
;                         uninstaller is written to for signing, and
;                         where the signed copy is found on the next
;                         compile (scripts/signpath.sh)
;
; The setup itself is signed after it is compiled; see the release
; workflow's Windows signing step.

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
; The executables' own (crates/greycard-ui/build.rs): the setup's file
; properties, nowhere in the wizard. Its company is AppPublisher's.
; This file is UTF-8 without a BOM, which Inno reads since 6.3.0.
VersionInfoCopyright=Copyright © Jess Olmstead. GPL-3.0.

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
; install and of uninstall, as install.ps1 tells it; Inno makes both
; calls itself.
ChangesEnvironment=yes
ChangesAssociations=yes

SetupIconFile={#AddBackslash(SourcePath)}..\..\assets\icon\greycard.ico
UninstallDisplayIcon={app}\greycard-ui.exe
UninstallDisplayName=greycard
WizardStyle=modern dynamic
Compression=lzma2/max
SolidCompression=yes

#ifdef SignedUninstallerDir
SignedUninstaller=yes
SignedUninstallerDir={#SignedUninstallerDir}
#endif

[Files]
; bin\ lands as the install directory itself, as install.ps1 copies
; it, and the three documents install.ps1 copies beside it. Not
; register.cmd and unregister.cmd, which install.ps1 copies too: the
; [Registry] entries below do their work here. ignoreversion: the
; script overwrites whatever is there, and so does this.
Source: "{#Stage}\bin\*"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY-NOTICES"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; install.ps1 (0.11.0's zip, and the zip since) copies the two
; scripts here, and so did this setup's first builds, which also ran
; them. Over either they go: the registration they wrote is the one
; [Registry] below writes again, key for key, and takes out on
; uninstall. A first build's uninstall log still holds the line that
; ran unregister.cmd, which no later setup can take back; with the
; script gone it fails, hidden, and changes nothing.
Type: files; Name: "{app}\register.cmd"
Type: files; Name: "{app}\unregister.cmd"

[UninstallDelete]
; And if a zip's install.ps1 put them back over this install since,
; they go on uninstall too, so the directory does not stay for them.
Type: files; Name: "{app}\register.cmd"
Type: files; Name: "{app}\unregister.cmd"

[Icons]
; greycard.lnk in the Start menu's Programs folder, no group, as
; install.ps1 makes it. The icon is the executable's own.
Name: "{autoprograms}\greycard"; Filename: "{app}\greycard-ui.exe"; WorkingDir: "{app}"; Comment: "A raw editor that gets the light right"

[Registry]
; The associations, as register.cmd writes them and unregister.cmd
; takes them out: under HKCU\Software\Classes, for this user only.
; The raw extensions are register.cmd's list, and the browser's;
; scripts/windows-setup-check.ps1 checks this install against
; register.cmd's, so the copies cannot drift apart unseen.
;
; Our own keys go whole on uninstall (uninsdeletekey). Where a key is
; shared, only our value goes: another application's offer for .cr3
; lives in the same OpenWithProgids key, which goes only when that
; leaves it empty. The offers are empty strings where register.cmd
; writes REG_NONE, which [Registry] cannot write; Windows reads only
; their names. The uninstaller works through these lines in reverse,
; so a subkey is emptied before its parent is looked at.
;
; Doubled quotes inside a quoted parameter are one quote: the command
; is "<app>\greycard-ui.exe" "%1".

; The application, so Open with shows a name rather than the file.
Root: HKCU; Subkey: "Software\Classes\Applications\greycard-ui.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "greycard"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\greycard-ui.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\greycard-ui.exe"" ""%1"""

; The sidecar: its own type, its own icon, and the extension. .gcd's
; default is taken back by [Code] below, and only while it still
; names our type; the key goes once nothing else is in it.
Root: HKCU; Subkey: "Software\Classes\greycard.gcd"; ValueType: string; ValueName: ""; ValueData: "greycard edit sidecar"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\greycard.gcd\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\greycard-ui.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\greycard.gcd\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\application-x-greycard-edit.ico"""
Root: HKCU; Subkey: "Software\Classes\.gcd"; ValueType: string; ValueName: ""; ValueData: "greycard.gcd"; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.gcd\OpenWithProgids"; ValueType: string; ValueName: "greycard.gcd"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty

; The frames: offered on each raw extension, never taken.
Root: HKCU; Subkey: "Software\Classes\greycard.photo"; ValueType: string; ValueName: ""; ValueData: "greycard photo"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\greycard.photo\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\greycard-ui.exe"",0"
Root: HKCU; Subkey: "Software\Classes\greycard.photo\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\greycard-ui.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.cr3\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.cr2\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.dng\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.nef\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.arw\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.raf\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.orf\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.rw2\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.pef\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\.ori\OpenWithProgids"; ValueType: string; ValueName: "greycard.photo"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty

[Run]
Filename: "{app}\greycard-ui.exe"; Description: "{cm:LaunchProgram,greycard}"; Flags: nowait postinstall skipifsilent

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

// .gcd's default, taken back only while it names our type: if
// something else has taken .gcd since, its choice stays, as
// unregister.cmd leaves it. Run before the uninstaller's own registry
// work, so the key's uninsdeletekeyifempty then finds it empty.
procedure ForgetSidecarExtension;
var
  Value: String;
begin
  if RegQueryStringValue(HKCU, 'Software\Classes\.gcd', '', Value) and
     SameText(Value, 'greycard.gcd') then
    RegDeleteValue(HKCU, 'Software\Classes\.gcd', '');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddToPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then begin
    RemoveFromPath;
    ForgetSidecarExtension;
  end;
end;
