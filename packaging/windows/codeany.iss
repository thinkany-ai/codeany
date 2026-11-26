; Inno Setup script for the Windows installer. Built in CI:
;   iscc /DVersion=<version> /DSource=<dir with codeany.exe> packaging\windows\codeany.iss
#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Source
  #define Source "..\..\target\release"
#endif

[Setup]
AppId={{6F4C2E8B-4D1A-4E8B-9C55-2B7A1D0C0DE1}
AppName=CodeAny
AppVersion={#Version}
AppPublisher=ThinkAny, LLC
AppPublisherURL=https://codeany.ai
DefaultDirName={autopf}\CodeAny
DefaultGroupName=CodeAny
DisableProgramGroupPage=yes
; Per-user by default, so installing needs no administrator.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\..\target\packages
OutputBaseFilename=CodeAny_{#Version}_x64_setup
SetupIconFile=..\..\target\packages\codeany.ico
UninstallDisplayIcon={app}\codeany.ico
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern

[Files]
Source: "{#Source}\codeany.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\packages\codeany.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\CodeAny"; Filename: "{app}\codeany.exe"; IconFilename: "{app}\codeany.ico"
Name: "{autodesktop}\CodeAny"; Filename: "{app}\codeany.exe"; IconFilename: "{app}\codeany.ico"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
Filename: "{app}\codeany.exe"; Description: "{cm:LaunchProgram,CodeAny}"; Flags: nowait postinstall skipifsilent
