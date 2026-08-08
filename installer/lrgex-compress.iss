; LRGEX Compress - Inno Setup installer
; Dual-mode install (HKA registry = HKLM when admin, HKCU when per-user).
; PrivilegesRequiredOverridesAllowed lets Chocolatey pass /ALLUSERS for machine-wide,
; while the portable download runs per-user with no UAC. Right-click only - no Start Menu.
;
; ONE cascading right-click entry ("LRGEX"):
;   right-click a FOLDER -> "LRGEX" -> hover -> "Compress"  -> folder.zgx
;   right-click any FILE  -> "LRGEX" -> hover -> "Compress"  -> file.zgx
;   right-click a .zgx    -> "LRGEX" -> hover -> "Extract"   -> <name>\ folder
;   (double-click a .zgx also extracts)
;
; TO COMPILE: "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\lrgex-compress.iss

#define MyAppName      "LRGEX Compress"
#define MyAppVersion "1.4.4"
#define MyAppPublisher "LRGEX"
#define MyAppExeName   "lrgex-compress.exe"

[Setup]
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\{#MyAppExeName}
VersionInfoVersion={#MyAppVersion}.0
DefaultDirName={autopf}\LRGEX Compress
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2
SolidCompression=yes
; deploy.ps1 overrides this at compile time via ISCC's /F flag, producing
; LRGEX-Compress-v<major>.<minor>-setup.exe. This line is a fallback for manual compiles.
OutputBaseFilename=lrgex-compress-{#MyAppVersion}-setup
OutputDir=../target/installer
CloseApplications=force
RestartApplications=no
ChangesEnvironment=yes
SetupIconFile=..\assets\icon.ico

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Ensure the ContextMenus parent keys are fully removed on uninstall (no orphan shells).
Root: HKA; Subkey: "Software\Classes\Directory\ContextMenus\LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\ContextMenus\LRGEX"; Flags: uninsdeletekey

; ===== FOLDER: cascade "LRGEX" -> hover -> "Compress" =====
Root: HKA; Subkey: "Software\Classes\Directory\shell\LRGEX"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\LRGEX"; \
    ValueType: string; ValueName: "MUIVerb"; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\LRGEX"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\Directory\shell\LRGEX"; \
    ValueType: string; ValueName: "ExtendedSubCommandsKey"; ValueData: "Directory\ContextMenus\LRGEX"
Root: HKA; Subkey: "Software\Classes\Directory\ContextMenus\LRGEX\shell\compress"; \
    ValueType: string; ValueName: ""; ValueData: "Compress"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\ContextMenus\LRGEX\shell\compress"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\Directory\ContextMenus\LRGEX\shell\compress\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""

; ===== FILES (all): cascade "LRGEX" -> hover -> "Compress" =====
Root: HKA; Subkey: "Software\Classes\*\shell\LRGEX"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\LRGEX"; \
    ValueType: string; ValueName: "MUIVerb"; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\LRGEX"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\*\shell\LRGEX"; \
    ValueType: string; ValueName: "ExtendedSubCommandsKey"; ValueData: "*\ContextMenus\LRGEX"
Root: HKA; Subkey: "Software\Classes\*\ContextMenus\LRGEX\shell\compress"; \
    ValueType: string; ValueName: ""; ValueData: "Compress"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\ContextMenus\LRGEX\shell\compress"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\*\ContextMenus\LRGEX\shell\compress\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""

; ===== .zgx: cascade "LRGEX" -> hover -> "Extract" (+ double-click = extract) =====
Root: HKA; Subkey: "Software\Classes\.zgx"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX.zgx"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX Compress Archive"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\DefaultIcon"; \
    ValueType: string; ValueData: "{app}\icon.ico,0"
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\shell\open\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" -x ""%1"""
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\shell\LRGEX"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\shell\LRGEX"; \
    ValueType: string; ValueName: "MUIVerb"; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\shell\LRGEX"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\shell\LRGEX"; \
    ValueType: string; ValueName: "ExtendedSubCommandsKey"; ValueData: "LRGEX.zgx\ContextMenus\LRGEX"
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extract"; \
    ValueType: string; ValueName: ""; ValueData: "Extract"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extract"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extract\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" -x ""%1"""
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extractHere"; \
    ValueType: string; ValueName: ""; ValueData: "Extract Here"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extractHere"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\LRGEX.zgx\ContextMenus\LRGEX\shell\extractHere\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" -x -h ""%1"""

; ===== .zip and .rar: cascade ONLY under SystemFileAssociations =====
; Do NOT take ownership of .zip/.rar — preserves Explorer's built-in zip handler and
; WinRAR/7-Zip if installed. Adds "LRGEX -> Extract" to their right-click menu only.
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\LRGEX"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\LRGEX"; \
    ValueType: string; ValueName: "MUIVerb"; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\LRGEX"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\LRGEX"; \
    ValueType: string; ValueName: "ExtendedSubCommandsKey"; ValueData: "LRGEX.ContextMenus\LRGEX"

Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\LRGEX"; \
    ValueType: string; ValueName: ""; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\LRGEX"; \
    ValueType: string; ValueName: "MUIVerb"; ValueData: "LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\LRGEX"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\LRGEX"; \
    ValueType: string; ValueName: "ExtendedSubCommandsKey"; ValueData: "LRGEX.ContextMenus\LRGEX"

; Shared submenu used by both .zip and .rar above.
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extract"; \
    ValueType: string; ValueName: ""; ValueData: "Extract"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extract"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extract\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" -x ""%1"""
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extractHere"; \
    ValueType: string; ValueName: ""; ValueData: "Extract Here"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extractHere"; \
    ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"
Root: HKA; Subkey: "Software\Classes\LRGEX.ContextMenus\LRGEX\shell\extractHere\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" -x -h ""%1"""

[Code]
const
  SHCNE_ASSOCCHANGED = $08000000;
  SHCNF_IDLIST = $0000;

procedure SHChangeNotify(wEventId: Integer; uFlags: Integer; dwItem1: Longint; dwItem2: Longint);
external 'SHChangeNotify@shell32.dll stdcall';

// Add the app directory to PATH. Uses HKLM when installed as admin (choco/machine-wide),
// HKCU when installed per-user (portable). Inno's HKA handles the shell verbs;
// PATH needs the explicit ternary because HKA doesn't apply to [Code] RegWrite calls.
// Guards against duplicates: if the path is already there (e.g., from a previous install),
// we don't add it again.
// Uses REG_EXPAND_SZ so any embedded %variables% in PATH expand correctly.
procedure AddAppToPath();
var
  CurrentPath: string;
  AppDir: string;
  PathRoot: LongInt; // HKEY_LOCAL_MACHINE or HKEY_CURRENT_USER
begin
  AppDir := ExpandConstant('{app}');
  if IsAdminInstallMode() then
    PathRoot := HKEY_LOCAL_MACHINE
  else
    PathRoot := HKEY_CURRENT_USER;

  if not RegQueryStringValue(PathRoot, 'Environment', 'Path', CurrentPath) then begin
    // No PATH exists yet for this user — create it.
    RegWriteExpandStringValue(PathRoot, 'Environment', 'Path', AppDir);
    exit;
  end;
  // Check for duplicate (case-insensitive, wrapping in semicolons to handle edge cases).
  if Pos(';' + UpperCase(AppDir) + ';', ';' + UpperCase(CurrentPath) + ';') > 0 then
    exit; // Already in PATH — don't duplicate.
  // Append to PATH (ensure a semicolon separator).
  if (CurrentPath <> '') and (CurrentPath[Length(CurrentPath)] <> ';') then
    CurrentPath := CurrentPath + ';';
  RegWriteExpandStringValue(PathRoot, 'Environment', 'Path', CurrentPath + AppDir);
end;

// Remove the app directory from the user's PATH on uninstall.
// Reads PATH, removes ONLY the app entry (case-insensitive), preserves everything else.
procedure RemoveAppFromPath();
var
  CurrentPath: string;
  AppDir: string;
  NewPath: string;
  Found: Boolean;
  PathRoot: LongInt;
begin
  AppDir := ExpandConstant('{app}');
  if IsAdminInstallMode() then
    PathRoot := HKEY_LOCAL_MACHINE
  else
    PathRoot := HKEY_CURRENT_USER;

  if not RegQueryStringValue(PathRoot, 'Environment', 'Path', CurrentPath) then
    exit; // No PATH to remove from.
  // Split PATH into parts by semicolon, filter out the app dir, rejoin.
  NewPath := '';
  Found := False;
  // Split manually (Inno Setup doesn't have a built-in split)
  CurrentPath := CurrentPath + ';'; // ensure trailing delimiter for clean parsing
  while Pos(';', CurrentPath) > 0 do begin
    if UpperCase(Copy(CurrentPath, 1, Pos(';', CurrentPath) - 1)) = UpperCase(AppDir) then
      Found := True // skip this entry
    else if Copy(CurrentPath, 1, Pos(';', CurrentPath) - 1) <> '' then begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Copy(CurrentPath, 1, Pos(';', CurrentPath) - 1);
    end;
    Delete(CurrentPath, 1, Pos(';', CurrentPath));
  end;
  if not Found then exit; // App wasn't in PATH — nothing to do.
  // Write back the cleaned PATH.
  if NewPath = '' then
    RegDeleteValue(PathRoot, 'Environment', 'Path')
  else
    RegWriteExpandStringValue(PathRoot, 'Environment', 'Path', NewPath);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    AddAppToPath();
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, 0, 0);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then begin
    RemoveAppFromPath();
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, 0, 0);
  end;
end;

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
