; Inno Setup script for RV ImgAuth (built by .github/workflows/release.yml).
; Per-user install: no administrator rights needed on the examiner's PC.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\.."
#endif

[Setup]
AppId={{E4BE67C2-9062-4A24-A9E0-12B16317933C}
AppName=RV ImgAuth
AppVersion={#AppVersion}
AppPublisher=Rakvor
AppPublisherURL=https://github.com/Panacota96/rv-imgauth
DefaultDirName={localappdata}\Programs\Rakvor\RV ImgAuth
DefaultGroupName=Rakvor
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#SourceDir}\LICENSE
OutputBaseFilename=rv-imgauth-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile={#SourceDir}\assets\brand\rv.ico
ChangesEnvironment=yes
UninstallDisplayIcon={app}\rv-imgauth-gui.exe

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "addtopath"; Description: "Add the rv-imgauth command to PATH"; Flags: checkedonce
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\target\release\rv-imgauth.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\target\release\rv-imgauth-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"
Source: "{#SourceDir}\NOTICE"; DestDir: "{app}"
Source: "{#SourceDir}\CHANGELOG.md"; DestDir: "{app}"
Source: "{#SourceDir}\docs\*"; DestDir: "{app}\docs"; Flags: recursesubdirs
Source: "{#SourceDir}\examples\sample\evidence\*"; DestDir: "{app}\examples\sample\evidence"

[Icons]
Name: "{group}\RV ImgAuth"; Filename: "{app}\rv-imgauth-gui.exe"
Name: "{group}\RV ImgAuth documentation"; Filename: "{app}\docs"
Name: "{autodesktop}\RV ImgAuth"; Filename: "{app}\rv-imgauth-gui.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\rv-imgauth-gui.exe"; Description: "Start RV ImgAuth"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') = 0;
end;
