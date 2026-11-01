; Inno Setup 6 script for the grimalkin Windows installer.
;
; Build (from the repository root, after staging dist\, see the "windows
; installer" job in .github/workflows/ci.yml):
;
;   ISCC.exe /DAppVersion=0.1.0 /DAppVersionNumeric=0.1.0.0 /DArch=x64 installer\windows\grimalkin.iss
;
; /DArch=x64 (the default) or /DArch=arm64 selects the architecture of the
; installer. dist\ must then hold binaries of that architecture, including the
; matching conpty.dll/OpenConsole.exe pair (CI verifies the machine types).
; Requires Inno Setup 6.3 or newer for the "arm64" architecture identifier.
;
; Inputs (all in <repo>\dist, relative to this file: ..\..\dist):
;   grimalkin.exe, grimalkin.com (the console launcher, renamed
;   grimalkin-launcher.exe), conpty.dll, OpenConsole.exe, LICENSE-ConPTY.txt,
;   README.md, LICENSE, LICENSE-MIT, LICENSE-APACHE
; Output: <repo>\installer-out\grimalkin-setup-<Arch>.exe (CI overrides the name
; with ISCC /F, e.g. grimalkin-setup-<Arch>-unsigned.exe)
;
; The setup/wizard icon is <repo>\assets\icon\grimalkin.ico. If that file is
; ever missing the build falls back to Inno's default icon (see HaveIcon
; below), so this script builds either way.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#ifndef AppVersionNumeric
  #define AppVersionNumeric "0.0.0.0"
#endif

#ifndef Arch
  #define Arch "x64"
#endif
#if (Arch != "x64") && (Arch != "arm64")
  #error Arch must be x64 or arm64 (/DArch=x64 or /DArch=arm64)
#endif

; Publisher shown in Apps & features. CI passes /DAppPublisher=<certificate common
; name> for a signed release and "grimalkin contributors (unsigned build)" for
; an unsigned one.
#ifndef AppPublisher
  #define AppPublisher "grimalkin contributors"
#endif

#define AppName "grimalkin"
#define AppExe "grimalkin.exe"
#define IconPath "..\..\assets\icon\grimalkin.ico"
#if FileExists(AddBackslash(SourcePath) + IconPath)
  #define HaveIcon
#endif

[Setup]
; Fixed identity: never change this GUID, it is how upgrades find the old
; install. ("{{" is Inno's escape for a literal "{".)
AppId={{5E56C746-3735-4FF7-96B9-E90F7D4C5AA7}
AppName={#AppName}
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersionNumeric}
VersionInfoProductTextVersion={#AppVersion}
; Explicit so Setup and the uninstaller carry the same ProductName as the program
; files ("Grimalkin"); SignPath's artifact configuration enforces it.
VersionInfoProductName=Grimalkin
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL=https://github.com/rlankenau/grimalkin
AppSupportURL=https://github.com/rlankenau/grimalkin/issues
AppUpdatesURL=https://github.com/rlankenau/grimalkin/releases

; Per-user by default (no UAC prompt); the wizard offers an "install for all
; users" choice, and /ALLUSERS or /CURRENTUSER pick one from the command line.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
; {autopf} is {localappdata}\Programs for a per-user install and
; {commonpf} (Program Files) for an all-users one.
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UsePreviousAppDir=yes

; x64: any x64-capable Windows, including x64 emulation on Windows 11 ARM64.
; arm64: native Windows ARM64 only. One AppId is shared by both architectures
; on purpose (one entry in Apps & features, one install dir, one PATH entry);
; the [Code] section below stops the two from being mixed.
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

WizardStyle=modern
Compression=lzma2/ultra
SolidCompression=yes

; Release signing. CI passes /DSignInstaller together with
; /Sgrimalkin=<command that signs $f> (see scripts/windows/build-installer.ps1
; and sign-file.ps1) only in the protected "windows sign" job. Inno then signs
; Setup AND the generated uninstaller (SignedUninstaller) before the uninstaller
; is embedded, which a signature added to the finished Setup could not do. Without
; /DSignInstaller nothing here changes and no signing tool is needed.
#ifdef SignInstaller
SignTool=grimalkin
SignedUninstaller=yes
SignToolRetryCount=3
#endif

; SignPath route (no Inno SignTool; see docs/SIGNING-SIGNPATH.md). SignPath cannot
; be called from inside ISCC, so the uninstaller is signed in a separate step:
; /DPreSignedUninstaller=<dir> enables SignedUninstaller without SignTool. The
; first compile writes the unsigned uninstaller into <dir>\uninst-<inno
; version>-<hash>.e32 (.e64) and stops with "please attach your digital signature to the
; following executable file"; once that file is signed (by SignPath), compiling
; again embeds its signature. The file name is a hash of the unsigned uninstaller,
; so a signature can never be attached to a different one.
#ifdef PreSignedUninstaller
  #ifdef SignInstaller
    #error SignInstaller and PreSignedUninstaller are different signing routes; define only one
  #endif
SignedUninstaller=yes
SignedUninstallerDir={#PreSignedUninstaller}
#endif

OutputDir=..\..\installer-out
OutputBaseFilename=grimalkin-setup-{#Arch}
#ifdef HaveIcon
SetupIconFile={#IconPath}
#endif
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\..\dist\LICENSE

; Upgrade in place: same AppId, close a running grimalkin (Restart Manager)
; so its files can be replaced. Do not relaunch it automatically.
CloseApplications=yes
RestartApplications=no
; Broadcasts GmSettingChangeMsg after install/uninstall when the PATH task ran.
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add grimalkin to PATH (so you can run ""grimalkin"" from any shell)"; GroupDescription: "Integration:"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Open grimalkin here"" to the Explorer folder context menu"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
Source: "..\..\dist\grimalkin.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\grimalkin.com"; DestDir: "{app}"; Flags: ignoreversion
; ConPTY redistributable: conpty.dll must sit next to grimalkin.exe with
; OpenConsole.exe beside it (see crates/pty/src/win/conpty.rs).
Source: "..\..\dist\conpty.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OpenConsole.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\LICENSE-ConPTY.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[UninstallDelete]
; Font index cache only; config and skins in %APPDATA%\grimalkin are kept.
Type: files; Name: "{localappdata}\grimalkin\fonts.cache"
Type: dirifempty; Name: "{localappdata}\grimalkin"

[Icons]
; WorkingDir: start in the user's profile rather than the install folder.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Registry]
; --- Architecture marker -----------------------------------------------------
; Records which architecture this install is, so a later installer of the other
; architecture can detect it (see InitializeSetup). Installs from before this
; value existed are x64 by definition. HKA: HKCU for per-user, HKLM for all-users.
Root: HKA; Subkey: "Software\grimalkin"; Flags: uninsdeletekeyifempty
Root: HKA; Subkey: "Software\grimalkin\Install"; ValueType: string; ValueName: "Arch"; ValueData: "{#Arch}"; Flags: uninsdeletekey

; --- PATH -------------------------------------------------------------------
; Appends {app} to the existing Path value only if it is not already there
; (NeedsAddPath), keeping the value's REG_EXPAND_SZ type. Per-user installs
; edit HKCU\Environment; all-users installs edit the machine Path. Removal is
; done in [Code] (CurUninstallStepChanged) since Inno has no "remove one
; element from a list value" flag.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: (not IsAdminInstallMode) and NeedsAddPath(False)
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: IsAdminInstallMode and NeedsAddPath(True)

; --- Explorer context menu --------------------------------------------------
; HKA is HKCU for a per-user install (HKCU\Software\Classes\...) and HKLM for
; an all-users one. The keys are deleted on uninstall.
; %V is quoted in the command line. A path that ends in a backslash (a drive
; or UNC root such as C:\ or \\srv\share\) would turn the closing quote into an
; escaped literal quote under CommandLineToArgvW rules, so "%V" is written as
; "%V." (C:\. is the same directory and never ends in a backslash).
; Right-click on a folder:
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin"; ValueType: string; ValueName: ""; ValueData: "Open grimalkin here"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --working-directory ""%V."""; Tasks: contextmenu
; Right-click on the empty background inside an open folder:
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin"; ValueType: string; ValueName: ""; ValueData: "Open grimalkin here"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --working-directory ""%V."""; Tasks: contextmenu

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; WorkingDir: "{%USERPROFILE}"; Flags: nowait postinstall skipifsilent

[Code]
const
  ThisArch = '{#Arch}';
  AppUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{5E56C746-3735-4FF7-96B9-E90F7D4C5AA7}_is1';
  ArchKey = 'Software\grimalkin\Install';
  UserEnvKey = 'Environment';
  MachineEnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  GmBroadcastHwnd = $FFFF;
  GmSettingChangeMsg = $001A;
  GmAbortIfHung = $0002;

function SendMessageTimeout(hWnd: LongWord; Msg: LongWord; wParam: LongWord;
  lParam: String; fuFlags: LongWord; uTimeout: LongWord;
  var lpdwResult: LongWord): LongWord;
  external 'SendMessageTimeoutW@user32.dll stdcall';

{ Tell running programs (Explorer, new shells) that the environment changed.
  ChangesEnvironment=yes does this too; the explicit call covers the manual
  edit done on uninstall. }
procedure BroadcastEnvironmentChange;
var
  Res: LongWord;
begin
  SendMessageTimeout(GmBroadcastHwnd, GmSettingChangeMsg, 0, 'Environment',
    GmAbortIfHung, 5000, Res);
end;

{ Returns the registry root and key holding the Path for this install mode. }
procedure GetPathLocation(Machine: Boolean; var Root: Integer; var Key: String);
begin
  if Machine then
  begin
    Root := HKEY_LOCAL_MACHINE;
    Key := MachineEnvKey;
  end
  else
  begin
    Root := HKEY_CURRENT_USER;
    Key := UserEnvKey;
  end;
end;

function TrimBackslash(const S: String): String;
begin
  Result := S;
  while (Length(Result) > 0) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

{ True if Path does not yet contain Dir. Entries are compared
  case-insensitively and ignoring a trailing backslash. }
function NeedsAddPath(Machine: Boolean): Boolean;
var
  Root: Integer;
  Dir, Key, OrigPath, Rest, Entry: String;
  P: Integer;
begin
  Dir := ExpandConstant('{app}');
  GetPathLocation(Machine, Root, Key);
  if not RegQueryStringValue(Root, Key, 'Path', OrigPath) then
  begin
    Result := True;
    Exit;
  end;
  Dir := Lowercase(TrimBackslash(Dir));
  Rest := OrigPath;
  Result := True;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end
    else
    begin
      Entry := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end;
    if Lowercase(TrimBackslash(Trim(Entry))) = Dir then
    begin
      Result := False;
      Exit;
    end;
  end;
end;

{ Removes every entry equal to Dir from the Path value, preserving all other
  entries and the REG_EXPAND_SZ type. Returns True if the value changed. }
function RemoveFromPath(Dir: String; Machine: Boolean): Boolean;
var
  Root: Integer;
  Key, OrigPath, NewPath, Rest, Entry: String;
  P: Integer;
begin
  Result := False;
  GetPathLocation(Machine, Root, Key);
  if not RegQueryStringValue(Root, Key, 'Path', OrigPath) then
    Exit;
  Dir := Lowercase(TrimBackslash(Dir));
  NewPath := '';
  Rest := OrigPath;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end
    else
    begin
      Entry := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end;
    if Lowercase(TrimBackslash(Trim(Entry))) = Dir then
      Result := True
    else if Entry <> '' then
    begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Entry;
    end;
  end;
  if Result then
    RegWriteExpandStringValue(Root, Key, 'Path', NewPath);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Changed: Boolean;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // The app folder is still resolvable here. Edit the Path of the mode this
    // install used; nothing happens if the entry is absent.
    if IsAdminInstallMode then
      Changed := RemoveFromPath(ExpandConstant('{app}'), True)
    else
      Changed := RemoveFromPath(ExpandConstant('{app}'), False);
    if Changed then
      BroadcastEnvironmentChange;
  end;
end;

{ --- x64 <-> ARM64 coexistence ---------------------------------------------
  Both architectures share one AppId, install directory, PATH entry and
  context-menu keys, so a second install of the other architecture (in the same
  or the other install mode) could leave a mix of files or registrations behind.
  Policy: an existing install of the OTHER architecture is removed first, via
  its own uninstaller. Interactive setup asks; silent setup refuses unless
  /REPLACEARCH=1 is passed. User settings (%APPDATA%\grimalkin) are never
  touched. Same-architecture installs are plain in-place upgrades.

  Security rule: setup never runs an uninstall command that came from a
  user-writable location (HKCU, or a per-user folder) while it is elevated.
  An unelevated process can write HKCU, so doing that would let it run any
  program as administrator. Therefore:
    - Not elevated: either kind of install may be replaced. A per-user
      uninstaller runs at the same privilege as the setup; an all-users one
      shows its own UAC prompt.
    - Elevated (setup is an administrator, e.g. /ALLUSERS): only an all-users
      install (HKLM, writable by administrators only) whose uninstaller sits in
      Program Files is replaced. A per-user install of the other architecture is
      DETECTED but never executed: setup stops and asks for it to be removed
      first (Settings, Apps) or for setup to be run again without
      administrator rights.
    - Known limitation: when a standard user elevates with a DIFFERENT
      administrator's credentials, HKCU is that administrator's hive, so the
      initiating user's own per-user install is not seen. Such an install of the
      other architecture must be removed by that user beforehand.
  Every uninstaller is validated before it runs (see CheckUninstaller). }

{ True when setup runs with administrator rights. The CITestUnelevated define
  makes a CI-ONLY build (never shipped) that reports False: the CI runner is an
  administrator, so this lets the smoke test exercise the unelevated per-user
  replacement flow. The release installer is built without it. }
function RunningElevated: Boolean;
begin
#ifdef CITestUnelevated
  Result := False;
#else
  Result := IsAdmin;
#endif
end;

{ Architecture recorded by an install under Root ('' = no install there). The
  uninstall key proves an install exists; the Arch value (missing on installs
  made before it was introduced) says which kind: missing means x64. }
function InstalledArch(Root: Integer): String;
begin
  Result := '';
  if RegKeyExists(Root, AppUninstallKey) then
  begin
    if not RegQueryStringValue(Root, ArchKey, 'Arch', Result) or (Result = '') then
      Result := 'x64';
  end;
end;

{ True if Path is inside Parent (a folder), compared case-insensitively. }
function IsUnderDir(Path, Parent: String): Boolean;
begin
  Path := Lowercase(Path);
  Parent := Lowercase(TrimBackslash(Parent)) + '\';
  Result := (Length(Path) > Length(Parent)) and (Copy(Path, 1, Length(Parent)) = Parent);
end;

{ Reads and validates the uninstaller of the install under Root. On success Cmd
  is the full path of the uninstaller and Dir its folder. Requirements:
    - UninstallString is exactly one quoted path (no extra arguments);
    - the path has no ".." part, exists, and is named unins*.exe;
    - it lies directly in the recorded InstallLocation;
    - when Elevated, it also lies inside Program Files. }
function CheckUninstaller(Root: Integer; Elevated: Boolean; var Cmd, Dir, Why: String): Boolean;
var
  Raw, Loc: String;
begin
  Result := False;
  Why := '';
  if not RegQueryStringValue(Root, AppUninstallKey, 'UninstallString', Raw) then
  begin
    Why := 'no UninstallString is recorded';
    Exit;
  end;
  Raw := Trim(Raw);
  if (Length(Raw) < 3) or (Raw[1] <> '"') or (Raw[Length(Raw)] <> '"') then
  begin
    Why := 'the UninstallString is not a single quoted path';
    Exit;
  end;
  Cmd := Copy(Raw, 2, Length(Raw) - 2);
  if (Pos('"', Cmd) > 0) or (Pos('..', Cmd) > 0) then
  begin
    Why := 'the UninstallString contains unexpected characters';
    Exit;
  end;
  Dir := ExtractFileDir(Cmd);
  if (Copy(Lowercase(ExtractFileName(Cmd)), 1, 5) <> 'unins') or
     (Lowercase(ExtractFileExt(Cmd)) <> '.exe') or not FileExists(Cmd) then
  begin
    Why := 'the uninstaller is not an existing unins*.exe file';
    Exit;
  end;
  if not RegQueryStringValue(Root, AppUninstallKey, 'InstallLocation', Loc) or
     (Lowercase(TrimBackslash(Loc)) <> Lowercase(TrimBackslash(Dir))) then
  begin
    Why := 'the uninstaller is not in the recorded install folder';
    Exit;
  end;
  if Elevated and not (IsUnderDir(Cmd, ExpandConstant('{commonpf64}')) or
                       IsUnderDir(Cmd, ExpandConstant('{commonpf32}'))) then
  begin
    Why := 'the uninstaller is not inside Program Files';
    Exit;
  end;
  Result := True;
end;

{ Runs the (already validated) uninstaller to completion; an all-users one
  shows its own UAC prompt when this setup is not elevated. _?= makes it run in
  place so we really wait. }
function UninstallExisting(Cmd, Dir: String; Silent: Boolean): Boolean;
var
  Params: String;
  Rc: Integer;
begin
  Result := False;
  Params := '/NORESTART /SUPPRESSMSGBOXES "_?=' + Dir + '"';
  if Silent then
    Params := '/VERYSILENT ' + Params
  else
    Params := '/SILENT ' + Params;
  Log('Removing other-architecture install: ' + Cmd + ' ' + Params);
  if ShellExec('open', Cmd, Params, '', SW_SHOW, ewWaitUntilTerminated, Rc) then
    Result := (Rc = 0)
  else
    Log('Could not start the uninstaller: ' + SysErrorMessage(Rc));
  { With _?= the uninstaller leaves itself and its folder behind. This installer
    overwrites unins000.* when it installs; if it is aborted instead, the stale
    files are harmless and have no registration left. }
end;

{ Returns True when it is fine to continue. PerUser says Root is HKCU. }
function ResolveOtherArch(Root: Integer; PerUser: Boolean; Where: String): Boolean;
var
  Found, Msg, Cmd, Dir, Why: String;
  Elevated: Boolean;
begin
  Result := True;
  Found := InstalledArch(Root);
  if (Found = '') or (Found = ThisArch) then
    Exit;
  Elevated := RunningElevated;
  Log(Format('Found an other-architecture install (%s) %s', [Found, Where]));
  Msg := Format('grimalkin (%s) is already installed %s. This installer is for ' +
    '%s, and the two cannot be installed together.', [Found, Where, ThisArch]);

  { Elevated and per-user: never execute it (the command comes from HKCU, which
    an unelevated process can write). Stop and say what to do. }
  if Elevated and PerUser then
  begin
    Log('Refusing to run a per-user uninstaller from an elevated setup: ' + Msg);
    if not WizardSilent then
      MsgBox(Msg + #13#10#13#10 + 'This setup is running with administrator rights, so it will ' +
        'not run the uninstaller of a per-user copy. Uninstall grimalkin from Settings, Apps, ' +
        'or run this installer again without administrator rights (choose "Install for me ' +
        'only"), then repeat.', mbError, MB_OK);
    Result := False;
    Exit;
  end;

  if not CheckUninstaller(Root, Elevated, Cmd, Dir, Why) then
  begin
    Log('Not running the uninstaller of the ' + Found + ' install: ' + Why);
    if not WizardSilent then
      MsgBox(Msg + #13#10#13#10 + 'Its uninstaller could not be used automatically (' + Why +
        '). Remove it from Settings, Apps, then run this installer again.', mbError, MB_OK);
    Result := False;
    Exit;
  end;

  if WizardSilent then
  begin
    if ExpandConstant('{param:REPLACEARCH|0}') <> '1' then
    begin
      Log(Msg + ' Pass /REPLACEARCH=1 to uninstall it first.');
      Result := False;
      Exit;
    end;
  end
  else if MsgBox(Msg + #13#10#13#10 + 'Uninstall the ' + Found + ' version now and continue? ' +
    'Your settings are kept.', mbConfirmation, MB_YESNO) <> IDYES then
  begin
    Result := False;
    Exit;
  end;
  Result := UninstallExisting(Cmd, Dir, WizardSilent);
  if not Result then
  begin
    Log('Uninstalling the ' + Found + ' install failed');
    if not WizardSilent then
      MsgBox('The ' + Found + ' version could not be uninstalled. Remove it from ' +
        'Settings, Apps, then run this installer again.', mbError, MB_OK);
  end;
end;

function InitializeSetup: Boolean;
begin
  Result := ResolveOtherArch(HKCU64, True, 'for the current user');
  if Result then
    Result := ResolveOtherArch(HKLM64, False, 'for all users');
end;
