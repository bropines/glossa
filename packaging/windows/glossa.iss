; Glossa's Windows installer, for Inno Setup 6.3 or newer.
;
;   iscc /DMyAppVersion=1.2.3 /DStageDir=<folder> /O<out-dir> packaging\windows\glossa.iss
;
; StageDir is the unpacked release zip: glossa.exe, glossa-cli.exe, the
; documents, examples\modules and sdk\python (Glossa looks for sdk\python
; beside the executable). Everything in it is installed as is.
;
; - Per user by default, into %LOCALAPPDATA%\Programs\Glossa, no admin
;   rights; the first page lets an administrator install for all users.
; - Upgrades in place: a running Glossa from this folder is closed first and,
;   after a silent upgrade, started again in the tray.
; - "Start with Windows" is the same HKCU Run value Glossa's own option
;   writes (glossa-platform/src/autostart.rs), so the two always agree.
; - Uninstalling keeps the settings in %APPDATA%\Glossa unless the user says
;   to delete them. The installer never creates a portable layout.

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#if Ver < EncodeVer(6, 3, 0)
  #error Inno Setup 6.3 or newer is needed (for x64compatible)
#endif
#ifndef StageDir
  #error Pass /DStageDir=<folder holding glossa.exe and the rest>
#endif
; The processor the executables in StageDir are built for: x64 (default) or
; arm64. An arm64 installer runs only on ARM Windows; the x64 one installs
; there too, under emulation.
#ifndef Arch
  #define Arch "x64"
#endif
#if Arch == "arm64"
  #define ArchAllowed "arm64"
  #define ArchName "arm64"
#elif Arch == "x64"
  #define ArchAllowed "x64compatible"
  #define ArchName "x86_64"
#else
  #error Arch must be x64 or arm64
#endif

; Never change this: Windows knows the installed program by it.
#define AppGuid "6101466B-422B-46CE-9B38-3DE5D1C61D58"
#define RepoDir AddBackslash(SourcePath) + "..\.."
; 1.2.3-rc.1 -> 1.2.3 for the numeric file version.
#define NumericVersion Copy(MyAppVersion, 1, Pos("-", MyAppVersion + "-") - 1)

[Setup]
AppId={{{#AppGuid}}
AppName=Glossa
AppVersion={#MyAppVersion}
AppVerName=Glossa {#MyAppVersion}
AppPublisher=Bropines
AppPublisherURL=https://github.com/bropines/glossa
AppSupportURL=https://github.com/bropines/glossa/issues
AppUpdatesURL=https://github.com/bropines/glossa/releases
VersionInfoVersion={#NumericVersion}
VersionInfoProductName=Glossa
VersionInfoDescription=Glossa installer
DefaultDirName={autopf}\Glossa
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; The Run value lives under HKCU even in an all-users install, as Glossa's
; own option keeps it.
UsedUserAreasWarning=no
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}
MinVersion=10.0
OutputBaseFilename=glossa-{#MyAppVersion}-{#ArchName}-windows-setup
SetupIconFile={#RepoDir}\assets\icons\glossa.ico
UninstallDisplayIcon={app}\glossa.exe
UninstallDisplayName=Glossa
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Glossa lives in the tray and keeps its executable open. The Restart
; Manager asks it to close and, if that does not work, ends it.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
en.StartupGroup=Startup:
ru.StartupGroup=Автозапуск:
en.AutostartTask=Start Glossa when I sign in to Windows
ru.AutostartTask=Запускать Glossa при входе в Windows
en.DeleteUserData=Also delete your Glossa settings, history and installed modules?%n%n%1%n%nChoose No to keep them for a later installation.
ru.DeleteUserData=Удалить также настройки, историю и установленные модули Glossa?%n%n%1%n%nВыберите «Нет», чтобы сохранить их для следующей установки.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:AutostartTask}"; GroupDescription: "{cm:StartupGroup}"; Flags: unchecked

[InstallDelete]
; Examples and the SDK are replaced as a whole, so files dropped from a
; release do not linger.
Type: filesandordirs; Name: "{app}\examples"
Type: filesandordirs; Name: "{app}\sdk"

[Files]
; A portable marker or data folder would move the settings beside the
; executable; keep them out even if the staging folder has one.
Source: "{#StageDir}\*"; DestDir: "{app}"; Excludes: "\portable,\portable.*,\data"; \
  Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Glossa"; Filename: "{app}\glossa.exe"
Name: "{autodesktop}\Glossa"; Filename: "{app}\glossa.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\glossa.exe"; Description: "{cm:LaunchProgram,Glossa}"; \
  Flags: nowait postinstall skipifsilent runasoriginaluser
; A silent upgrade (winget, Scoop's updater) brings back a Glossa it closed.
Filename: "{app}\glossa.exe"; Parameters: "--hidden"; \
  Flags: nowait runasoriginaluser; Check: RestartAfterSilentUpgrade

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';
  RunValue = 'Glossa';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{{#AppGuid}}_is1';

var
  IsUpgrade: Boolean;
  PreviousDir: String;
  WasRunning: Boolean;
  TasksPreset: Boolean;

function ExeIn(Dir: String): String;
begin
  Result := AddBackslash(Dir) + 'glossa.exe';
end;

{ Whether the Run value starts the glossa.exe in Dir. }
function AutostartPointsTo(Dir: String): Boolean;
var
  Command: String;
begin
  Result := RegQueryStringValue(HKCU, RunKey, RunValue, Command) and
    (Pos(Lowercase(ExeIn(Dir)), Lowercase(Command)) > 0);
end;

function PsQuote(S: String): String;
begin
  Result := S;
  StringChangeEx(Result, '''', '''''', True);
  Result := '''' + Result + '''';
end;

{ Runs PowerShell over the Glossa processes started from Exe, and only
  those: a portable copy elsewhere is left alone. Stops them when Stop is
  set; otherwise returns whether there are any. }
function GlossaProcesses(Exe: String; Stop: Boolean): Boolean;
var
  Script: String;
  Code: Integer;
begin
  Script := '$p = Get-Process -Name glossa -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -eq ' + PsQuote(Exe) + ' }; ';
  if Stop then
    Script := Script + 'if ($p) { $p | Stop-Process -Force; Start-Sleep -Milliseconds 500 }; exit 0'
  else
    Script := Script + 'if ($p) { exit 1 } else { exit 0 }';
  Result := Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + Script + '"',
    '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 1);
end;

function InitializeSetup(): Boolean;
begin
  IsUpgrade := RegQueryStringValue(HKCU, UninstallKey, 'Inno Setup: App Path', PreviousDir) or
    RegQueryStringValue(HKLM, UninstallKey, 'Inno Setup: App Path', PreviousDir);
  WasRunning := IsUpgrade and GlossaProcesses(ExeIn(PreviousDir), False);
  Result := True;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  { The Restart Manager asks first and only in a visible install; a silent
    one closes a running Glossa from the target folder here. }
  if WizardSilent then
    GlossaProcesses(ExeIn(ExpandConstant('{app}')), True);
  Result := '';
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  { On an upgrade the autostart box shows what is in force now, which the
    user may have changed in Glossa's options since the last install. }
  if (CurPageID = wpSelectTasks) and IsUpgrade and not TasksPreset then
  begin
    TasksPreset := True;
    if AutostartPointsTo(PreviousDir) then
      WizardSelectTasks('autostart')
    else
      WizardSelectTasks('!autostart');
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  WasOn, Want: Boolean;
begin
  if CurStep <> ssPostInstall then
    exit;
  WasOn := AutostartPointsTo(ExpandConstant('{app}')) or (IsUpgrade and AutostartPointsTo(PreviousDir));
  { A silent upgrade has no tasks page: keep what is in force. }
  if WizardSilent and IsUpgrade then
    Want := WasOn
  else
    Want := WizardIsTaskSelected('autostart');
  if Want then
  begin
    RegWriteStringValue(HKCU, RunKey, RunValue, '"' + ExeIn(ExpandConstant('{app}')) + '" --hidden');
    { Turned on here, so clear a "disabled" left by Task Manager, as
      Glossa's own option does. }
    if not WasOn then
      RegDeleteValue(HKCU, ApprovedKey, RunValue);
  end
  else if WasOn then
    RegDeleteValue(HKCU, RunKey, RunValue);
end;

function RestartAfterSilentUpgrade(): Boolean;
begin
  Result := WizardSilent and WasRunning;
end;

function InitializeUninstall(): Boolean;
begin
  { Close this folder's Glossa so its files can go; always go on. }
  GlossaProcesses(ExpandConstant('{app}\glossa.exe'), True);
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir, CacheDir: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    if AutostartPointsTo(ExpandConstant('{app}')) then
    begin
      RegDeleteValue(HKCU, RunKey, RunValue);
      RegDeleteValue(HKCU, ApprovedKey, RunValue);
    end;
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    { Where glossa-core/src/paths.rs keeps an installed Glossa's files:
      settings, history and modules, and the cache with the log. }
    DataDir := ExpandConstant('{userappdata}\Glossa');
    CacheDir := ExpandConstant('{localappdata}\Glossa');
    if not UninstallSilent and (DirExists(DataDir) or DirExists(CacheDir)) then
      if MsgBox(FmtMessage(CustomMessage('DeleteUserData'), [DataDir]),
          mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      begin
        DelTree(DataDir, True, True, True);
        DelTree(CacheDir, True, True, True);
      end;
  end;
end;
