; crossh Windows installer (Inno Setup 6).
; Built by scripts/package-windows.ps1 from the same stage directory as the
; portable zip, so setup.exe and the zip always contain identical files.
;
; Required defines (passed via ISCC /D):
;   MyAppVersion  e.g. 0.30.1
;   MyArch        x86_64 | aarch64
;   MySrcDir      absolute path of the staged files directory
;   MyOutputDir   absolute path of the output directory (dist)
;   MySetupBase   output base filename, e.g. crossh-0.30.1-windows-x86_64-setup
;
; Design notes:
; - Per-user install into {localappdata}\Programs\crossh, no admin required.
;   The in-place auto-updater (crossh-updater.exe replacing exe files) keeps
;   working after an Inno install without elevation, because the install
;   location is version-independent.
; - The installer only handles first-time install. Later updates keep using
;   the zip channel in stable.json (exe replacement), never re-running setup.
; - Unsigned build: SmartScreen still warns, same as the portable zip.
;
; NOTE: AppId below is stable and must never change; it identifies the
; installed product for upgrades and uninstall.

#ifndef MyAppVersion
#define MyAppVersion "0.0.0"
#endif
#ifndef MyArch
#define MyArch "x86_64"
#endif
#ifndef MySrcDir
#define MySrcDir "dist\\stage"
#endif
#ifndef MyOutputDir
#define MyOutputDir "dist"
#endif
#ifndef MySetupBase
#define MySetupBase "crossh-setup"
#endif

#if MyArch == "aarch64"
#define MyArchAllowed "arm64"
#define MyArchMode "arm64"
#else
#define MyArchAllowed "x64compatible"
#define MyArchMode "x64compatible"
#endif

[Setup]
AppId={{3F6A1B2C-9D4E-4F7A-8B1C-D2E3F4A5B6C7}
AppName=crossh
AppVersion={#MyAppVersion}
AppVerName=crossh {#MyAppVersion}
AppPublisher=xcrong
AppPublisherURL=https://github.com/xcrong/crossh
AppSupportURL=https://github.com/xcrong/crossh/issues
AppUpdatesURL=https://github.com/xcrong/crossh/releases
DefaultDirName={localappdata}\Programs\crossh
PrivilegesRequired=lowest
ArchitecturesAllowed={#MyArchAllowed}
ArchitecturesInstallIn64BitMode={#MyArchMode}
OutputDir={#MyOutputDir}
OutputBaseFilename={#MySetupBase}
SetupIconFile={#SourcePath}\..\assets\appicon\AppIcon.ico
UninstallDisplayIcon={app}\crossh.exe
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
MinVersion=10.0
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes
DisableProgramGroupPage=yes
VersionInfoVersion={#MyAppVersion}
VersionInfoCompany=xcrong
VersionInfoDescription=crossh terminal workspace
VersionInfoProductName=crossh
VersionInfoProductVersion={#MyAppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
; ChineseSimplified.isl is missing from some Inno layouts (e.g. per-user
; installs); fall back to English-only instead of failing the compile.
#if FileExists(CompilerPath + "Languages\ChineseSimplified.isl")
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "envpath"; Description: "Add crossh to user PATH"; GroupDescription: "Other options:"; Flags: unchecked

[Files]
Source: "{#MySrcDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\crossh"; Filename: "{app}\crossh.exe"
Name: "{autodesktop}\crossh"; Filename: "{app}\crossh.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: envpath; Check: NeedsAddPath('{app}')

[Run]
Filename: "{app}\crossh.exe"; Description: "Launch crossh"; Flags: nowait postinstall skipifsilent unchecked

[Code]
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Param + ';', ';' + OrigPath + ';') = 0;
end;
