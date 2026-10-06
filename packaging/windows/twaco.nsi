; Windows installer for twaco: a per-user install that needs no administrator.
;
; WARNING: changes the user's PATH in HKCU\Environment. The uninstaller removes the same entry.
;
; The release workflow builds it:
;   makensis /DVERSION=1.2.3 /DEXE=C:\full\path\twaco.exe /DOUTFILE=C:\full\path\setup.exe packaging/windows/twaco.nsi
;
; makensis runs in the directory of this script, so give EXE and OUTFILE as absolute paths.
;
; The install puts twaco.exe in %LOCALAPPDATA%\Programs\twaco, adds that directory to the user's
; PATH, and registers an uninstaller in Settings > Apps.

!ifndef VERSION | EXE | OUTFILE
  !error "Define VERSION, EXE and OUTFILE on the makensis command line"
!endif

; makensis runs in the directory of this script.
!define ROOT "../.."
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\twaco"

Unicode true
Name "twaco ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\twaco"
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "twaco"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "twaco installer"
VIAddVersionKey "LegalCopyright" "MIT License"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!define MUI_ICON "${ROOT}/assets/icon/twaco.ico"
!define MUI_UNICON "${ROOT}/assets/icon/twaco.ico"
!define MUI_FINISHPAGE_TEXT "twaco is on your PATH. Open a new terminal and run: twaco --help"
!insertmacro MUI_PAGE_LICENSE "${ROOT}/LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Add or remove $INSTDIR in the user's PATH. PowerShell edits the registry value directly:
; NSIS strings truncate a long PATH, and [Environment]::SetEnvironmentVariable expands the
; %VARIABLE% entries in it. The directory goes through the TWACO_DIR environment variable, so
; no character in it can break the command. Usage: !insertmacro EditPath "add"
!macro EditPath ACTION
  System::Call 'Kernel32::SetEnvironmentVariable(t "TWACO_DIR", t "$INSTDIR")i'
  nsExec::ExecToLog `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$ErrorActionPreference = 'Stop'; $$d = $$env:TWACO_DIR; $$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$p = [string]$$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); $$parts = @($$p -split ';' | Where-Object { $$_ -and $$_.TrimEnd('\') -ne $$d.TrimEnd('\') }); if ('${ACTION}' -eq 'add') { $$parts += $$d }; $$k.SetValue('Path', ($$parts -join ';'), 'ExpandString')"`
  Pop $0
  ${If} $0 != 0
    DetailPrint "PATH was not changed: PowerShell gave $0"
    MessageBox MB_OK|MB_ICONEXCLAMATION "twaco could not change your PATH (${ACTION} $INSTDIR). Edit PATH by hand in Settings > System > About > Advanced system settings > Environment Variables." /SD IDOK
  ${EndIf}
  ; Tell running programs, such as Explorer, that the environment changed.
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

Section "twaco"
  SetOutPath "$INSTDIR"
  File "/oname=twaco.exe" "${EXE}"
  File "/oname=LICENSE.txt" "${ROOT}/LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  !insertmacro EditPath "add"

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "twaco"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "butteredstardust"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/butteredstardust/twaco"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\twaco.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  !insertmacro EditPath "remove"
  Delete "$INSTDIR\twaco.exe"
  ; Remove a staged binary that an interrupted `twaco update` left.
  Delete "$INSTDIR\.twaco*"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
