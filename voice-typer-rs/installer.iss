; Voice Typer (Rust) — Inno Setup script
; Compile: iscc installer.iss → produces installer-out\VoiceTyper-Setup.exe

#define AppName       "Voice Typer"
#define AppVersion    "0.4.4"
#define AppPublisher  "Voice Typer"
#define AppURL        "https://voice.codag.site"
#define ExeName       "VoiceTyper.exe"

[Setup]
AppId={{B6F12C44-7A1A-4D03-9B95-21B0E7F11A77}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
OutputDir=installer-out
OutputBaseFilename=VoiceTyper-Setup
SetupIconFile=assets\logo.ico
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
CloseApplications=force
RestartApplications=no

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english";             MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon";        Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: checkedonce
Name: "startmenuicon";      Description: "Criar atalho no menu Iniciar"; GroupDescription: "{cm:AdditionalIcons}"; Flags: checkedonce
Name: "startupwithwindows"; Description: "Iniciar o Voice Typer junto com o Windows"; GroupDescription: "Inicialização:"; Flags: unchecked

[Files]
Source: "target\release\{#ExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "assets\logo.ico";            DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userdesktop}\{#AppName}";       Filename: "{app}\{#ExeName}"; IconFilename: "{app}\logo.ico"; Tasks: desktopicon
Name: "{group}\{#AppName}";             Filename: "{app}\{#ExeName}"; IconFilename: "{app}\logo.ico"; Tasks: startmenuicon
Name: "{group}\Site oficial";           Filename: "{#AppURL}";        Tasks: startmenuicon
Name: "{group}\Desinstalar {#AppName}"; Filename: "{uninstallexe}";   Tasks: startmenuicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
  ValueName: "{#AppName}"; ValueData: """{app}\{#ExeName}"""; \
  Tasks: startupwithwindows; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#ExeName}"; Description: "Executar {#AppName} agora"; \
  Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "taskkill.exe"; Parameters: "/F /IM {#ExeName}"; Flags: runhidden; RunOnceId: "killApp"

[UninstallDelete]
Type: filesandordirs; Name: "{userappdata}\VoiceTyper"
Type: filesandordirs; Name: "{%USERPROFILE}\.cache\voice-typer"
