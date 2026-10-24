; The Windows installer (Inno Setup 6): per-user, no administrator needed, the launcher, the game and QWC Studio
; with the files the remaster ships. Built by the release workflow (docs/RELEASING.md), or by hand:
;
;   python packaging/stage.py --bin target/release --out target/stage --notices target/THIRD-PARTY-LICENSES.html
;   iscc /DAppVersion=0.1.0 /DStage=..\..\target\stage /DOutDir=..\..\target\dist packaging\windows\qwc-remastered.iss
;
; It writes QWCRemastered-<version>-setup.exe, the name the launcher's updater looks for. Installing over an older
; version replaces the programs and the shipped files; the player's library (the imported game, packs, saves'
; backups, settings) is in their profile and is never touched, not even by uninstalling.
;
; The setup of the player's game is part of the installation ([Code]): their game folder and their no-CD QWC.exe,
; each checked by the launcher (`check-game`, `check-exe`) on Next, then imported into the library (`import`) once
; the programs are installed. A library already set up is kept unless the player asks to set it up again; "Later"
; leaves it to the launcher's own setup. Silent installs import with /GAME="folder" /EXE="file".

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
; the launcher again, unpacked to a temporary folder for the setup pages' checks before anything is installed
Source: "{#Stage}\qwc-launcher.exe"; Flags: dontcopy

[Icons]
Name: "{autoprograms}\QWC Remastered"; Filename: "{app}\qwc-launcher.exe"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: startmenu
Name: "{autoprograms}\QWC Studio"; Filename: "{app}\qwc-studio.exe"; Parameters: "--packs"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: startmenu
Name: "{autodesktop}\QWC Remastered"; Filename: "{app}\qwc-launcher.exe"; WorkingDir: "{app}"; IconFilename: "{app}\qwc.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\qwc-launcher.exe"; Description: "{cm:LaunchProgram,QWC Remastered}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
const
  // where the game's own installer puts it (the launcher's setup looks there first too)
  UsualGame = 'C:\Program Files (x86)\EA GAMES\Harry Potter - Quidditch WC';

var
  ChoicePage: TInputOptionWizardPage;
  GamePage: TInputDirWizardPage;
  ExePage: TInputFileWizardPage;
  Checking: TNewStaticText;
  AlreadySetUp: Boolean;
  Checker: String;

// The library: $QWC_LIBRARY, else %APPDATA%\QWCRemastered (qwc_packs::library_dir).
function LibraryDir: String;
begin
  Result := GetEnv('QWC_LIBRARY');
  if Result = '' then
    Result := ExpandConstant('{userappdata}\QWCRemastered');
end;

// A path the profile (profile.ron) gives as `Key: Some("...")`, or ''.
function ProfileValue(const Text, Key: String): String;
var
  P: Integer;
  Rest: String;
begin
  Result := '';
  P := Pos(Key + ': Some("', Text);
  if P = 0 then
    Exit;
  Rest := Copy(Text, P + Length(Key) + 8, MaxInt);
  P := Pos('")', Rest);
  if P = 0 then
    Exit;
  Result := Copy(Rest, 1, P - 1);
  StringChangeEx(Result, '\\', '\', True);
end;

// A path as one argument: quoted, without a trailing backslash that would escape the quote.
function Quoted(const Path: String): String;
begin
  Result := RemoveBackslashUnlessRoot(Path);
  if Copy(Result, Length(Result), 1) = '\' then
    Result := Result + '.';
  Result := '"' + Result + '"';
end;

// What a command printed (its first lines), for a message.
function Said(const Output: TExecOutput): String;
var
  I: Integer;
begin
  Result := '';
  for I := 0 to GetArrayLength(Output.StdOut) - 1 do
    if I < 12 then
      Result := Result + Output.StdOut[I] + #13#10;
  for I := 0 to GetArrayLength(Output.StdErr) - 1 do
    if I < 6 then
      Result := Result + Output.StdErr[I] + #13#10;
  Result := Trim(Result);
end;

// The launcher's `Command Path` ("Checking..." on Page meanwhile): whether the setup can use it (exit 0; 3 is
// no), and what it said.
function Check(Page: TWizardPage; const Command, Path: String; var Text: String): Boolean;
var
  Code: Integer;
  Output: TExecOutput;
begin
  if Checker = '' then
  begin
    ExtractTemporaryFile('qwc-launcher.exe');
    Checker := ExpandConstant('{tmp}\qwc-launcher.exe');
  end;
  Checking.Parent := Page.Surface;
  Checking.Visible := True;
  WizardForm.NextButton.Enabled := False;
  WizardForm.Refresh;
  try
    Result := ExecAndCaptureOutput(Checker, Command + ' ' + Quoted(Path), '', SW_HIDE, ewWaitUntilTerminated, Code,
      Output) and (Code = 0);
    Text := Said(Output);
    if Code <> 0 then
      Log(Format('%s %s: exit %d', [Command, Path, Code]));
  finally
    Checking.Visible := False;
    WizardForm.NextButton.Enabled := True;
  end;
end;

// Whether the game is set up during this installation.
function ImportWanted: Boolean;
begin
  if WizardSilent then
    Result := (ExpandConstant('{param:game|}') <> '') and (ExpandConstant('{param:exe|}') <> '')
  else if AlreadySetUp then
    Result := ChoicePage.SelectedValueIndex = 1
  else
    Result := ChoicePage.SelectedValueIndex = 0;
end;

procedure InitializeWizard;
var
  Profile: AnsiString;
  Data, Exe: String;
begin
  if LoadStringFromFile(LibraryDir + '\profile.ron', Profile) then
  begin
    Data := ProfileValue(String(Profile), 'data');
    Exe := ProfileValue(String(Profile), 'exe');
    AlreadySetUp := (Data <> '') and DirExists(Data) and FileExists(Exe);
  end;

  ChoicePage := CreateInputOptionPage(wpSelectDir, 'Your copy of the game',
    'QWC Remastered plays from your own copy of Quidditch World Cup.',
    'Nothing of the original game comes with the remaster. It reads your installed game (or a copy of the disc''s ' +
    'files) and a QWC.exe without copy protection that you provide, and copies them into its library in your ' +
    'user profile. Your game folder is not changed.', True, False);
  if AlreadySetUp then
  begin
    ChoicePage.Add('Keep the game already set up (' + Data + ')');
    ChoicePage.Add('Set it up again now');
  end
  else
  begin
    ChoicePage.Add('Set it up now (recommended)');
    ChoicePage.Add('Later, in the launcher');
  end;
  ChoicePage.SelectedValueIndex := 0;

  GamePage := CreateInputDirPage(ChoicePage.ID, 'Your game folder', 'Where is Quidditch World Cup installed?',
    'Choose the game''s folder: the one with Audio, Locale and QWC.exe in it, installed or copied from the disc. ' +
    'Its files are checked against the known editions when you click Next.', False, '');
  GamePage.Add('');
  GamePage.Values[0] := ExpandConstant('{param:game|}');
  if (GamePage.Values[0] = '') and DirExists(UsualGame) then
    GamePage.Values[0] := UsualGame;

  ExePage := CreateInputFilePage(GamePage.ID, 'Your no-CD QWC.exe', 'Which QWC.exe should the remaster read?',
    'The remaster reads tables from the game''s program, so it needs a QWC.exe without SafeDisc copy protection ' +
    '(a no-CD exe) from your own copy of the game. The retail QWC.exe from the disc is encrypted and cannot be ' +
    'read. It is checked when you click Next.');
  ExePage.Add('QWC.exe without copy protection:', 'Programs (*.exe)|*.exe|All files|*.*', '.exe');
  ExePage.Values[0] := ExpandConstant('{param:exe|}');

  Checking := TNewStaticText.Create(WizardForm);
  Checking.Caption := 'Checking...';
  Checking.Top := ScaleY(200);
  Checking.Visible := False;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := ((PageID = GamePage.ID) or (PageID = ExePage.ID)) and not ImportWanted;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  // the exe in the game folder to start with: often the no-CD one, where the game runs without the disc
  if (CurPageID = ExePage.ID) and (ExePage.Values[0] = '') and
    FileExists(AddBackslash(GamePage.Values[0]) + 'QWC.exe') then
    ExePage.Values[0] := AddBackslash(GamePage.Values[0]) + 'QWC.exe';
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Text: String;
  P: Integer;
begin
  Result := True;
  // silent: no questions; the import checks both again, and a failed one leaves the setup to the launcher
  if WizardSilent then
    Exit;
  if CurPageID = GamePage.ID then
  begin
    if not Check(GamePage, 'check-game', GamePage.Values[0], Text) then
    begin
      MsgBox('This folder can''t be used:' + #13#10#13#10 + Text, mbError, MB_OK);
      Result := False;
    end
    else if Pos('Verified', Text) <> 1 then
      Result := MsgBox(Text + #13#10#13#10 + 'Use this folder anyway?', mbConfirmation, MB_YESNO) = IDYES;
  end
  else if CurPageID = ExePage.ID then
  begin
    Result := Check(ExePage, 'check-exe', ExePage.Values[0], Text);
    // the verdict, without the hash and the tables' count after it
    P := Pos('  sha256', Text);
    if P > 0 then
      Text := Copy(Text, 1, P - 1);
    if not Result then
      MsgBox('This QWC.exe can''t be used:' + #13#10#13#10 + Text, mbError, MB_OK)
    else if Pos('Verified', Text) <> 1 then
      Result := MsgBox(Text + #13#10#13#10 + 'Use it anyway?', mbConfirmation, MB_YESNO) = IDYES;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
  Output: TExecOutput;
  Ok: Boolean;
begin
  if (CurStep <> ssPostInstall) or not ImportWanted then
    Exit;
  WizardForm.StatusLabel.Caption := 'Importing your game files into the remaster''s library...';
  WizardForm.FilenameLabel.Caption := '';
  WizardForm.ProgressGauge.Style := npbstMarquee;
  try
    Ok := ExecAndCaptureOutput(ExpandConstant('{app}\qwc-launcher.exe'),
      'import ' + Quoted(ExePage.Values[0]) + ' ' + Quoted(GamePage.Values[0]), ExpandConstant('{app}'), SW_HIDE,
      ewWaitUntilTerminated, Code, Output) and (Code = 0);
    Log('import: ' + Said(Output));
    if not Ok then
      SuppressibleMsgBox('Your game files were not imported:' + #13#10#13#10 + Said(Output) + #13#10#13#10 +
        'The launcher will ask for them when it starts.', mbError, MB_OK, IDOK);
  finally
    WizardForm.ProgressGauge.Style := npbstNormal;
  end;
end;
