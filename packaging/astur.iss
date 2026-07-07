; Astur (Full) installer — Inno Setup script.
;
; Builds a per-user setup.exe (no admin) that installs the window-manager exe
; + the settings-GUI exe, a Start-Menu shortcut, an optional run-at-sign-in
; shortcut, and an uninstaller. The portable exe (GitHub release) stays the
; alternative for people who don't want an installer; this is the friendly path.
;
; Build:  ISCC.exe packaging\astur.iss   (from the repo root; needs a prior
;         `cargo build --release` so target\release\*.exe exist)
; Output: dist\Astur-Setup-<version>.exe
;
; TODO (see plan/editions.md): embed a custom .ico in astur.exe (build.rs +
; embed-resource) so Explorer / taskbar / this shortcut show the Astur logo
; instead of the default exe icon. Not blocking for testing.

#define AppName "Astur"
#define AppVersion "2.0.0-alpha.1"
#define AppPublisher "Astur"
#define AppURL "https://astur.app"
#define AppExe "Astur.exe"

[Setup]
; Stable per-app GUID — do NOT change between versions (uninstall/upgrade keys off it).
AppId={{7C7F9E2A-4A1E-4B3D-9E2C-0A1B2C3D4E5F}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={autopf}\Astur
DefaultGroupName=Astur
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=Astur-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
; Per-user install — no admin prompt. Astur needs no elevation (running elevated
; only adds the ability to manage elevated windows; not required to install).
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
; If Astur is already running, close it before replacing the exe (reinstall loop).
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "startup"; Description: "Start Astur automatically when I sign in"; GroupDescription: "Startup:"

[Files]
; The window manager. Renamed to Astur.exe on install (release binary is astur.exe).
Source: "..\target\release\astur.exe"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
; The settings GUI (WIP stub for now) — the tray "Settings" item launches this sibling.
Source: "..\target\release\astur-settings.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Astur"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall Astur"; Filename: "{uninstallexe}"
; Optional autostart: a shortcut in the user's Startup folder (no registry, easy to see/remove).
Name: "{userstartup}\Astur"; Filename: "{app}\{#AppExe}"; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch Astur now"; Flags: nowait postinstall skipifsilent
