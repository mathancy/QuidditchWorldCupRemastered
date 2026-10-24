; The Windows installer (Inno Setup 6): per-user, no administrator needed, the launcher, the game and QWC Studio
; with the files the remaster ships. Built by the release workflow (docs/RELEASING.md), or by hand:
;
;   python packaging/stage.py --bin target/release --out target/stage --notices target/THIRD-PARTY-LICENSES.html
;   iscc /DAppVersion=0.1.0 /DStage=..\..\target\stage /DOutDir=..\..\target\dist packaging\windows\qwc-remastered.iss
;
; It writes QWCRemastered-<version>-setup.exe, the name the launcher's updater looks for. Installing over an older
; version replaces the programs and the shipped files; the player's library (the imported game, packs, saves'
; backups, settings) is in their profile and is never touched, not even by uninstalling.

#ifndef AppVersion
  #error Pass the version: /DAppVersion=X.Y.Z
#endif
#ifndef Stage
  #define Stage "..\..\target\stage"
#endif
#ifndef OutDir
  #define OutDir "..\..\target\dist"
#endif

[Setup]
; the same id for every version, so a new one installs over the old
AppId={{7C3F3FD0-0951-4DD3-820B-EA7B8A6CB7D9}
AppName=QWC Remastered
AppVersion={#AppVersion}
AppVerName=QWC Remastered {#AppVersion}
AppPublisher=The QWC Remastered contributors
AppPublisherURL=https://github.com/mathancy/QuidditchWorldCupRemastered
AppSupportURL=https://github.com/mathancy/QuidditchWorldCupRemastered/issues
AppUpdatesURL=https://github.com/mathancy/QuidditchWorldCupRemastered/releases
DefaultDirName={localappdata}\Programs\QWC Remastered
DefaultGroupName=QWC Remastered
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#Stage}\LICENSE
SetupIconFile=..\..\crates\qwc-style\assets\qwc.ico
UninstallDisplayIcon={app}\qwc.ico
UninstallDisplayName=QWC Remastered
OutputDir={#OutDir}
OutputBaseFilename=QWCRemastered-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; the launcher closes itself to update; anything else of ours still running is asked to close
CloseApplications=yes
RestartApplications=no

[Tasks]
; both shortcuts are the player's choice, ticked to start with
Name: "startmenu"; Description: "Create Start menu entries"; GroupDescription: "{cm:AdditionalIcons}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; the shipped files are replaced whole, so a layout or look a newer version dropped does not linger
Type: filesandordirs; Name: "{app}\crates"
Type: filesandordirs; Name: "{app}\remastered"

[Files]
Source: "{#Stage}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion
Source: "..\..\crates\qwc-style\assets\qwc.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\QWC Remastered"; Filename: "{app}\qwc-launcher.exe"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: startmenu
Name: "{autoprograms}\QWC Studio"; Filename: "{app}\qwc-studio.exe"; Parameters: "--packs"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: startmenu
Name: "{autodesktop}\QWC Remastered"; Filename: "{app}\qwc-launcher.exe"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\qwc-launcher.exe"; Description: "{cm:LaunchProgram,QWC Remastered}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
