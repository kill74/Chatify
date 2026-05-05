#define MyAppName "Chatify"

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#ifndef SourceDir
  #error SourceDir preprocessor variable is not defined. Pass /DSourceDir=... to ISCC.
#endif

#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{C2B89D0A-55D7-4E46-8F6A-8BE5E0E73570}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=kill74
AppPublisherURL=https://github.com/kill74/Chatify
AppSupportURL=https://github.com/kill74/Chatify
AppUpdatesURL=https://github.com/kill74/Chatify/releases
DefaultDirName={localappdata}\Programs\Chatify
DefaultGroupName=Chatify
DisableProgramGroupPage=yes
LicenseFile={#SourceDir}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=chatify-setup-{#MyAppVersion}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\chatify.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\chatify.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\chatify-server.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\chatify-client.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\chatify-launcher.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\start-server.bat"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\start-client.bat"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\start-chatify.bat"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Chatify\Chatify"; Filename: "{app}\chatify.exe"; WorkingDir: "{app}"
Name: "{autoprograms}\Chatify\Chatify Launcher (fallback)"; Filename: "{app}\chatify-launcher.cmd"; WorkingDir: "{app}"
Name: "{autoprograms}\Chatify\Host + Local Client"; Filename: "{app}\start-chatify.bat"; WorkingDir: "{app}"
Name: "{autoprograms}\Chatify\Start Server Only"; Filename: "{app}\start-server.bat"; WorkingDir: "{app}"
Name: "{autoprograms}\Chatify\Join Server"; Filename: "{app}\start-client.bat"; WorkingDir: "{app}"
Name: "{autodesktop}\Chatify"; Filename: "{app}\chatify.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\chatify.exe"; Description: "Launch Chatify"; Flags: nowait postinstall skipifsilent
