; Kuality installer (Inno Setup 6). See docs/specs/distribution.md.
;
; Build: scripts\package.ps1 -Installer   (or: ISCC.exe installer\kuality.iss after `cargo build --release`)
; Output: dist\Kuality-<version>-setup.exe
;
; Per-user install, no admin rights. Start at logon is set by kuality.exe itself
; (`--startup normal|elevated|off`), the same code as the settings window, so the Run value and the
; elevated scheduled task never coexist.

#define AppName "Kuality"
#define AppExe "kuality.exe"
#define SourceExe "..\target\release\kuality.exe"
#ifndef AppVersion
  #define AppVersion GetStringFileInfo(SourceExe, "ProductVersion")
#endif

[Setup]
AppId={{6C1C4F0E-2B7A-4C55-9E7B-4B2D7C1A9F31}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
DefaultDirName={userpf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
OutputDir=..\dist
OutputBaseFilename=Kuality-{#AppVersion}-setup
SetupIconFile=..\assets\icon\kuality.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Kuality is closed by the [Code] below first; Restart Manager is the fallback for files in use.
CloseApplications=yes
RestartApplications=no
; Unticked startup tasks leave the current entry alone: an upgrade must not undo a choice made later
; in the settings window, so previous selections are not reused.
UsePreviousTasks=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Opt-in like every Kuality feature. Unticked = leave the current startup entry as it is.
Name: "autostart"; Description: "Start Kuality with Windows"; GroupDescription: "Startup:"; Flags: unchecked
Name: "autostart\elevated"; Description: "Run as administrator at sign-in (Windows asks for permission once)"; GroupDescription: "Startup:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
; Opens the settings window (and starts Kuality if it is not running).
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "--settings"; Comment: "Kuality settings"

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "--startup normal"; StatusMsg: "Setting up start with Windows..."; Tasks: autostart and not autostart\elevated; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Parameters: "--startup elevated"; StatusMsg: "Setting up start as administrator..."; Tasks: autostart\elevated; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Parameters: "--settings"; Description: "Open Kuality"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; Removes the Run value and, if it exists, the elevated task (one UAC prompt only then).
Filename: "{app}\{#AppExe}"; Parameters: "--startup off"; RunOnceId: "RemoveStartup"; Flags: runhidden waituntilterminated

[Code]
const
  WM_CLOSE = $0010;
  CoreWindowClass = 'Kuality.Core';

{ Closes a running Kuality gracefully (its settings window follows) and waits up to 5 s. }
procedure CloseKuality();
var
  Wnd: HWND;
  I: Integer;
begin
  Wnd := FindWindowByClassName(CoreWindowClass);
  if Wnd = 0 then
    Exit;
  PostMessage(Wnd, WM_CLOSE, 0, 0);
  for I := 1 to 50 do
  begin
    Sleep(100);
    if FindWindowByClassName(CoreWindowClass) = 0 then
      Exit;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseKuality();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  CloseKuality();
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  DataDir := ExpandConstant('{userappdata}\Kuality');
  if not DirExists(DataDir) then
    Exit;
  { Settings are kept unless the user asks; never deleted in silent mode. }
  if UninstallSilent() then
    Exit;
  if SuppressibleMsgBox('Also delete your Kuality settings?' + #13#10#13#10 + DataDir,
       mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
    DelTree(DataDir, True, True, True);
end;
