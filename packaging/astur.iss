; Astur (Full) installer — Inno Setup script.
;
; Builds a per-user setup.exe (no admin) that installs the window-manager exe
; + the settings-GUI exe + the status bar (rsb.exe), a Start-Menu shortcut, an
; optional run-at-sign-in shortcut, and an uninstaller. The portable exe (GitHub
; release) stays the alternative for people who don't want an installer; this
; is the friendly path.
;
; Build:  ISCC.exe packaging\astur.iss   (from the repo root; needs a prior
;         `cargo build --release` in the root and in rsb\, so target\release\*.exe
;         and rsb\target\release\rsb.exe exist)
; Output: dist\Astur-Setup-<version>.exe
;
; (The hawk .ico is embedded in every exe via build.rs + embed-resource.)

#define AppName "Astur"
#define AppVersion "3.0.0-alpha.1"
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
; setup.exe's own version info (#30: no version info reads as suspect to AV).
; Numbers only (Inno rejects "2.5.0-beta.1"): read from astur.exe's VERSIONINFO.
VersionInfoVersion={#GetVersionNumbersString(SourcePath + "..\target\release\astur.exe")}
VersionInfoCopyright=Licensed under the Apache License 2.0
AppSupportURL={#AppURL}
DefaultDirName={autopf}\Astur
DefaultGroupName=Astur
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\LICENSE
SetupIconFile=..\crates\astur\assets\astur.ico
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
; The settings GUI — the tray "Settings" item and the power menu launch this sibling.
Source: "..\target\release\astur-settings.exe"; DestDir: "{app}"; Flags: ignoreversion
; The status bar. Never in Startup: only Astur goes there (Astur 3.0 will start
; RSB when RSB is the chosen bar).
; A running rsb.exe is closed by CloseApplications above, like Astur.
Source: "..\rsb\target\release\rsb.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Astur"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall Astur"; Filename: "{uninstallexe}"
; Optional autostart: a shortcut in the user's Startup folder (no registry, easy to see/remove).
Name: "{userstartup}\Astur"; Filename: "{app}\{#AppExe}"; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch Astur now"; Flags: nowait postinstall skipifsilent
