Unicode true
!include "MUI2.nsh"
!ifndef APP_VERSION
  !define APP_VERSION "0.5.0"
!endif

Name "DevSpace PC Bridge"
OutFile "DevSpacePCBridge-Setup.exe"
InstallDir "$LOCALAPPDATA\Programs\DevSpacePCBridge"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey "ProductName" "DevSpace PC Bridge"
VIAddVersionKey "FileVersion" "${APP_VERSION}"
VIAddVersionKey "LegalCopyright" "MIT"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "PortugueseBR"

Section "Instalar" Main
    SetOutPath "$INSTDIR"
    File "target\release\DevSpacePCBridge.exe"
    CreateDirectory "$SMPROGRAMS\DevSpace PC Bridge"
    CreateShortCut "$SMPROGRAMS\DevSpace PC Bridge\DevSpace PC Bridge.lnk" "$INSTDIR\DevSpacePCBridge.exe"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "DisplayName" "DevSpace PC Bridge"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "DisplayVersion" "${APP_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "Publisher" "OmPo"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge" "NoRepair" 1
SectionEnd

Section "Uninstall"
    Delete "$INSTDIR\DevSpacePCBridge.exe"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    Delete "$SMPROGRAMS\DevSpace PC Bridge\DevSpace PC Bridge.lnk"
    RMDir "$SMPROGRAMS\DevSpace PC Bridge"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\DevSpacePCBridge"
    ; Never remove settings, Owner credentials, DevSpace, Tailscale or projects.
SectionEnd
