; Instalador do IsNotAEngine (Inno Setup 6).
; Uso: iscc /DAppVersion=1.0.0 installer\IsNotAEngine.iss  (gera dist\IsNotAEngine-Setup-<versão>.exe)
; Pré-requisito: `cargo build --release` já executado.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{6F1D2C54-3B9A-4E57-9C0E-1A8B5D7E2F90}
AppName=IsNotAEngine
AppVersion={#AppVersion}
AppVerName=IsNotAEngine {#AppVersion}
AppPublisher=4TyllaL
DefaultDirName={autopf}\IsNotAEngine
DefaultGroupName=IsNotAEngine
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=IsNotAEngine-Setup-{#AppVersion}
SetupIconFile=..\assets\brand\icon.ico
UninstallDisplayIcon={app}\IsNotAEngine.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked
Name: "autostart"; Description: "Iniciar com o Windows"; Flags: unchecked

[Files]
Source: "..\target\release\IsNotAEngine.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
; Vídeos de fábrica (gerados à parte; a pasta pode estar vazia no repositório).
Source: "..\assets\wallpapers\videos\*"; Excludes: "README.md"; DestDir: "{app}\videos"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\IsNotAEngine Studio"; Filename: "{app}\IsNotAEngine.exe"; Parameters: "--studio"
Name: "{autodesktop}\IsNotAEngine"; Filename: "{app}\IsNotAEngine.exe"; Parameters: "--studio"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "IsNotAEngine"; ValueData: """{app}\IsNotAEngine.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\IsNotAEngine.exe"; Parameters: "--studio"; Description: "Abrir o IsNotAEngine Studio"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM IsNotAEngine.exe /F"; Flags: runhidden; RunOnceId: "StopEngine"
