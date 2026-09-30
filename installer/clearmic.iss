; ClearMic installer (Inno Setup 6). Build with: .\build.ps1 -Installer
;
; One installer for everything: the app plus the VB-CABLE virtual microphone driver.
; VB-CABLE is made by VB-Audio (www.vb-cable.com) and is donationware. VB-Audio allows
; embedding it with silent installation as long as the end user is told so (see
; https://vb-audio.com/Services/licensing.htm). Company-wide deployments need licences.
; Build with /DNoDriver to produce an installer without the driver.
#define AppName "ClearMic"
#define AppVersion "1.0.1"
#define AppExe "clearmic.exe"
#ifndef NoDriver
  #if FileExists(SourcePath + "vbcable\pack\VBCABLE_Setup_x64.exe")
    #define BundleDriver
  #endif
#endif

[Setup]
AppId={{8C5B7E0A-3B1E-4C5E-9F42-5D0C1A7E9B21}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=ClearMic
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
#ifdef BundleDriver
OutputBaseFilename=ClearMic-Setup-{#AppVersion}
#else
OutputBaseFilename=ClearMic-Setup-{#AppVersion}-nodriver
#endif
SetupIconFile=..\assets\clearmic.ico
UninstallDisplayIcon={app}\clearmic.ico
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
WizardStyle=modern
CloseApplications=no

[Tasks]
#ifdef BundleDriver
Name: "vbcable"; Description: "Install the virtual microphone driver: VB-CABLE by VB-Audio (www.vb-cable.com). VB-CABLE is donationware, all participations are welcome."; GroupDescription: "Virtual microphone (needed so other apps can use the clean audio):"; Check: not CableInstalled
#endif
Name: "autostart"; Description: "Start ClearMic when Windows starts"; GroupDescription: "Startup:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\x86_64-pc-windows-gnu\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\clearmic.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
#ifdef BundleDriver
; Unmodified VB-CABLE package, including its readme and licence.
Source: "vbcable\pack\*"; DestDir: "{app}\vbcable"; Flags: ignoreversion
#endif

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\clearmic.ico"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\clearmic.ico"; Tasks: desktopicon

[Registry]
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"" --minimized"; Flags: uninsdeletevalue; Tasks: autostart
#ifdef BundleDriver
Root: HKLM; Subkey: "Software\ClearMic"; ValueType: dword; ValueName: "InstalledVBCable"; ValueData: "1"; Flags: uninsdeletekey; Tasks: vbcable
#endif

[Run]
#ifdef BundleDriver
Filename: "{app}\vbcable\VBCABLE_Setup_x64.exe"; Parameters: "-i -h"; WorkingDir: "{app}\vbcable"; StatusMsg: "Installing the VB-CABLE virtual microphone driver (VB-Audio)..."; Flags: waituntilterminated; Tasks: vbcable
#endif
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{cmd}"; Parameters: "/C taskkill /IM {#AppExe} /F"; Flags: runhidden; RunOnceId: "StopClearMic"

[Code]
function CableInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SYSTEM\CurrentControlSet\Services\VBAudioVACMME');
end;

{ True when Windows already exposes the cable as a playback endpoint (no reboot needed). }
function CableEndpointPresent: Boolean;
var
  Names: TArrayOfString;
  I: Integer;
  S, Base: String;
begin
  Result := False;
  Base := 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render';
  if RegGetSubkeyNames(HKLM64, Base, Names) then
    for I := 0 to GetArrayLength(Names) - 1 do
      if RegQueryStringValue(HKLM64, Base + '\' + Names[I] + '\Properties',
           '{b3f8fa53-0004-438e-9003-51a46e139bfc},6', S) then
        if Pos('VB-Audio', S) > 0 then
        begin
          Result := True;
          Exit;
        end;
end;

{ ClearMic hides to the tray when its window is closed, so stop it explicitly before
  files are replaced during an upgrade. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{cmd}'), '/C taskkill /IM {#AppExe} /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
  Result := '';
end;

function NeedRestart: Boolean;
begin
  Result := False;
#ifdef BundleDriver
  if WizardIsTaskSelected('vbcable') then
    Result := not CableEndpointPresent;
#endif
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  V: Cardinal;
  Code: Integer;
  Setup: String;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  Setup := ExpandConstant('{app}\vbcable\VBCABLE_Setup_x64.exe');
  if RegQueryDWordValue(HKLM64, 'Software\ClearMic', 'InstalledVBCable', V) and (V = 1)
     and FileExists(Setup) then
    if SuppressibleMsgBox('ClearMic installed the VB-CABLE virtual audio driver. Remove it as well?',
         mbConfirmation, MB_YESNO, IDYES) = IDYES then
      Exec(Setup, '-u -h', ExtractFileDir(Setup), SW_HIDE, ewWaitUntilTerminated, Code);
end;
