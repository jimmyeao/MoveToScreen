; JumpScreen installer — builds JumpScreenSetup.exe from a release build.
; Requires Inno Setup 6 (iscc.exe).

#define AppName "JumpScreen"
#define AppExe "jumpscreen.exe"

[Setup]
AppName={#AppName}
AppVersion=0.1.0
AppPublisher=Jimmyeao
DefaultDirName={localappdata}\Programs\JumpScreen
DefaultGroupName={#AppName}
OutputDir=..\installer\out
OutputBaseFilename=JumpScreenSetup
PrivilegesRequired=lowest
SetupIconFile=..\jumpscreen.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\jumpscreen.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\jumpscreen.ico"

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "install"; \
    StatusMsg: "Adding JumpScreen tasks to app jump lists..."
Filename: "{app}\{#AppExe}"; Parameters: "guard"; \
    StatusMsg: "Scheduling periodic re-register..."

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "uninstall"; RunOnceId: "RemoveTasks"
Filename: "{app}\{#AppExe}"; Parameters: "guard --off"; RunOnceId: "RemoveSchedule"
