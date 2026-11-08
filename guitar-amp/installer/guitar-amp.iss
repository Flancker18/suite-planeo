; Script Inno Setup : crée GuitarAmp-Setup.exe
#define AppName "Guitar Amp"
#define AppVersion "0.1.0"

[Setup]
AppId={{7B1E5C2A-4D3F-4E8B-9A51-6C0F2D9A1E77}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\guitar-amp.exe
OutputDir=Output
OutputBaseFilename=GuitarAmp-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "Créer un raccourci sur le Bureau"; GroupDescription: "Raccourcis :"

[Files]
Source: "..\target\release\guitar-amp.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\guitar-amp.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\guitar-amp.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\guitar-amp.exe"; Description: "Lancer {#AppName}"; Flags: nowait postinstall skipifsilent
