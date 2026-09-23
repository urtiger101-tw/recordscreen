#define AppName "RecordScreen"
#define AppVersion "0.1.1"
#define AppPublisher "urtiger101-tw"
#define AppExeName "recordscreen.exe"
#define FFmpegUrl "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.7z"
#define FFmpegSha256 "4705843ccaaf54257c16ad90f3e952ece33c17df964ecf7bfdbb0f49c7171077"

[Setup]
AppId={{8D37C931-C5DD-4FD5-AC04-CE2CE83F699B}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\RecordScreen
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=RecordScreen-Setup-{#AppVersion}-x64
UninstallDisplayIcon={app}\{#AppExeName}
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
UsePreviousAppDir=yes
CloseApplications=force
RestartApplications=no
ArchiveExtraction=basic

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
Name: "downloadffmpeg"; Description: "Download FFmpeg from gyan.dev (~35 MB); required for capture"; GroupDescription: "Capture dependency:"; Check: not IsFFmpegAvailable

[Files]
Source: "..\target\release\recordscreen.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[UninstallDelete]
Type: filesandordirs; Name: "{app}\_ffmpeg_runtime"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: postinstall nowait skipifsilent

[Code]
var
  FFmpegDownloadPage: TDownloadWizardPage;
  FFmpegRuntimeCreatedBySetup: Boolean;
  SetupCompleted: Boolean;

function DirectoryHasFFmpeg(const Directory: String): Boolean;
var
  Path: String;
begin
  Path := RemoveQuotes(Trim(Directory));
  if Path = '' then
  begin
    Result := False;
    Exit;
  end;

  StringChangeEx(Path, '%USERPROFILE%', ExpandConstant('{userprofile}'), True);
  StringChangeEx(Path, '%LOCALAPPDATA%', ExpandConstant('{localappdata}'), True);
  StringChangeEx(Path, '%APPDATA%', ExpandConstant('{userappdata}'), True);
  StringChangeEx(Path, '%PROGRAMDATA%', ExpandConstant('{commonappdata}'), True);
  StringChangeEx(Path, '%PROGRAMFILES%', ExpandConstant('{autopf}'), True);
  StringChangeEx(Path, '%PROGRAMFILES(X86)%', ExpandConstant('{autopf32}'), True);
  Result := FileExists(Path + '\ffmpeg.exe');
end;

function PathHasFFmpeg(const PathValue: String): Boolean;
var
  Remaining, Directory: String;
  Separator: Integer;
begin
  Remaining := PathValue;
  while Remaining <> '' do
  begin
    Separator := Pos(';', Remaining);
    if Separator = 0 then
    begin
      Directory := Remaining;
      Remaining := '';
    end
    else
    begin
      Directory := Copy(Remaining, 1, Separator - 1);
      Delete(Remaining, 1, Separator);
    end;

    if DirectoryHasFFmpeg(Directory) then
    begin
      Result := True;
      Exit;
    end;
  end;

  Result := False;
end;

function IsFFmpegAvailable: Boolean;
var
  PathValue: String;
begin
  Result := DirectoryHasFFmpeg(ExpandConstant('{app}')) or
    FileExists(ExpandConstant('{app}\_ffmpeg_runtime\ffmpeg-9.0.2-essentials_build\bin\ffmpeg.exe'));
  if Result then
    Exit;

  Result := PathHasFFmpeg(GetEnv('PATH'));
  if Result then
    Exit;

  if RegQueryStringValue(HKCU, 'Environment', 'Path', PathValue) and PathHasFFmpeg(PathValue) then
  begin
    Result := True;
    Exit;
  end;

  if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', PathValue) and PathHasFFmpeg(PathValue) then
  begin
    Result := True;
    Exit;
  end;

  Result := DirectoryHasFFmpeg(ExpandConstant('{localappdata}\Microsoft\WinGet\Links')) or
    DirectoryHasFFmpeg(ExpandConstant('{userprofile}\scoop\shims')) or
    DirectoryHasFFmpeg(ExpandConstant('{userprofile}\scoop\apps\ffmpeg-essentials\current\bin')) or
    DirectoryHasFFmpeg(ExpandConstant('{userprofile}\scoop\apps\ffmpeg\current\bin')) or
    DirectoryHasFFmpeg(ExpandConstant('{commonappdata}\chocolatey\bin')) or
    DirectoryHasFFmpeg(ExpandConstant('{autopf}\ffmpeg\bin')) or
    DirectoryHasFFmpeg(ExpandConstant('{autopf32}\ffmpeg\bin'));
end;

procedure InitializeWizard;
begin
  FFmpegDownloadPage := CreateDownloadPage('Downloading FFmpeg', 'FFmpeg is required for screenshots and recording.', nil);
  FFmpegDownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Error: String;
begin
  Result := True;
  if (CurPageID = wpReady) and WizardIsTaskSelected('downloadffmpeg') and not IsFFmpegAvailable then
  begin
    FFmpegDownloadPage.Clear;
    FFmpegDownloadPage.Add('{#FFmpegUrl}', 'ffmpeg-essentials.7z', '{#FFmpegSha256}');
    FFmpegDownloadPage.Show;
    try
      try
        FFmpegDownloadPage.Download;
      except
        if FFmpegDownloadPage.AbortedByUser then
          Error := 'FFmpeg download was cancelled.'
        else
          Error := 'FFmpeg download failed: ' + GetExceptionMessage;
        SuppressibleMsgBox(Error, mbCriticalError, MB_OK, IDOK);
        Result := False;
      end;
    finally
      FFmpegDownloadPage.Hide;
    end;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  RuntimeDirectory: String;
begin
  Result := '';
  if not (WizardIsTaskSelected('downloadffmpeg') and not IsFFmpegAvailable) then
    Exit;

  RuntimeDirectory := ExpandConstant('{app}\_ffmpeg_runtime');
  if DirExists(RuntimeDirectory) then
  begin
    Result := 'The FFmpeg runtime folder already exists but no usable FFmpeg was found. Remove it or choose another install folder, then run Setup again.';
    Exit;
  end;

  if not ForceDirectories(RuntimeDirectory) then
  begin
    Result := 'Could not create the FFmpeg runtime folder.';
    Exit;
  end;
  FFmpegRuntimeCreatedBySetup := True;

  try
    ExtractArchive(ExpandConstant('{tmp}\ffmpeg-essentials.7z'), RuntimeDirectory, '', True, nil);
  except
    Result := 'Could not extract FFmpeg: ' + GetExceptionMessage;
    DelTree(RuntimeDirectory, True, True, True);
    FFmpegRuntimeCreatedBySetup := False;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssDone then
    SetupCompleted := True;
end;

procedure DeinitializeSetup;
begin
  if FFmpegRuntimeCreatedBySetup and not SetupCompleted then
    DelTree(ExpandConstant('{app}\_ffmpeg_runtime'), True, True, True);
end;
