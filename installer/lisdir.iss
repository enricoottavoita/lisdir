; Inno Setup script. Build: ISCC /DMyVersion=0.10.0 installer\lisdir.iss  (needs src-tauri\target\release\lisdir.exe)
#ifndef MyVersion
  #define MyVersion "0.0.0"
#endif

[Setup]
AppId={{8F1B7C3E-4D2A-4B59-9E6C-3A5D0C7F2B14}
AppName=lisdir
AppVersion={#MyVersion}
AppPublisher=lisdir contributors
DefaultDirName={autopf}\lisdir
DefaultGroupName=lisdir
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=lisdir-setup-{#MyVersion}
Compression=lzma2
SolidCompression=yes
; Admin is needed once: Program Files, and the elevated helper's scheduled task.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\lisdir.exe
WizardStyle=modern
ShowLanguageDialog=auto

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"

[CustomMessages]
english.Autostart=Start lisdir when I sign in to Windows
italian.Autostart=Avvia lisdir all'accesso a Windows
english.InstallingService=Installing the index service...
italian.InstallingService=Installazione del servizio di indicizzazione...
english.StartNow=Start lisdir now
italian.StartNow=Avvia lisdir ora

[Tasks]
Name: "autostart"; Description: "{cm:Autostart}"

[Files]
Source: "..\src-tauri\target\release\lisdir.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\lisdir"; Filename: "{app}\lisdir.exe"

[Run]
; Written as the user who launched the installer (not the elevated account), so it lands in the right HKCU.
Filename: "{sys}\reg.exe"; Parameters: "add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v lisdir /t REG_SZ /d ""\""{app}\lisdir.exe\"""" /f"; Flags: runhidden runasoriginaluser; Tasks: autostart
; Register and start the elevated index helper (scheduled task at logon), then open the app as the normal user.
Filename: "{app}\lisdir.exe"; Parameters: "--install"; Flags: runhidden waituntilterminated; StatusMsg: "{cm:InstallingService}"
Filename: "{app}\lisdir.exe"; Description: "{cm:StartNow}"; Flags: nowait postinstall skipifsilent runasoriginaluser
; Started by the in-app updater (silent install with /START=1).
Filename: "{app}\lisdir.exe"; Flags: nowait runasoriginaluser; Check: StartRequested

[Code]
function StartRequested: Boolean;
begin
  Result := ExpandConstant('{param:start|0}') = '1';
end;

// Stop a running copy (UI and elevated helper) so the exe can be replaced or removed.
procedure StopLisdir;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\schtasks.exe'), '/End /TN lisdir-helper', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM lisdir.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopLisdir;
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
begin
  if CurUninstallStep = usUninstall then
  begin
    StopLisdir;
    Exec(ExpandConstant('{sys}\schtasks.exe'), '/Delete /TN lisdir-helper /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'lisdir');
  end;
  if CurUninstallStep = usPostUninstall then
    DelTree(ExpandConstant('{localappdata}\lisdir'), True, True, True); // index caches and roots.txt
end;
