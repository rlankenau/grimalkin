; Inno Setup 6 script for the grimalkin Windows installer.
;
; Build (from the repository root, after staging dist\, see the "windows
; installer" job in .github/workflows/ci.yml):
;
;   ISCC.exe /DAppVersion=0.1.0 installer\windows\grimalkin.iss
;
; Inputs (all in <repo>\dist, relative to this file: ..\..\dist):
;   grimalkin.exe, grimalkin.com (the console launcher, renamed
;   grimalkin-launcher.exe), conpty.dll, OpenConsole.exe, LICENSE-ConPTY.txt,
;   README.md, LICENSE, LICENSE-MIT, LICENSE-APACHE
; Output: <repo>\installer-out\grimalkin-setup-x64.exe
;
; The setup/wizard icon is <repo>\assets\icon\grimalkin.ico. If that file is
; ever missing the build falls back to Inno's default icon (see HaveIcon
; below), so this script builds either way.

#ifndef AppVersion
  #define AppVersion "0.0.0"
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
AppVerName={#AppName} {#AppVersion}
AppPublisher=grimalkin contributors
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

ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

WizardStyle=modern
Compression=lzma2/ultra
SolidCompression=yes

OutputDir=..\..\installer-out
OutputBaseFilename=grimalkin-setup-x64
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

[Icons]
; WorkingDir: start in the user's profile rather than the install folder.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Registry]
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
; Right-click on a folder:
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin"; ValueType: string; ValueName: ""; ValueData: "Open grimalkin here"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\grimalkin\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --working-directory ""%V"""; Tasks: contextmenu
; Right-click on the empty background inside an open folder:
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin"; ValueType: string; ValueName: ""; ValueData: "Open grimalkin here"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\grimalkin\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --working-directory ""%V"""; Tasks: contextmenu

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; WorkingDir: "{%USERPROFILE}"; Flags: nowait postinstall skipifsilent

[Code]
const
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
