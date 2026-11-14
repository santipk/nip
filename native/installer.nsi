Unicode true
!include MUI2.nsh
!include nsDialogs.nsh
!include LogicLib.nsh
!include x64.nsh

!ifndef VERSION
  !define VERSION "0.2.0"
!endif
!ifndef BINARY
  !error "BINARY must identify the verified native release executable"
!endif
!ifndef OUTPUT
  !error "OUTPUT must identify the installer file"
!endif
!ifndef NOTICES
  !error "NOTICES must identify third-party license notices"
!endif
!ifdef TEST_BUILD
  !define PRODUCT "NIP Native Test"
  !define APP_EXE "NIPNativeTest.exe"
  !define REGKEY "Software\NIPNativeInstallerTest"
  !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\NIPNativeInstallerTest"
  InstallDir "$EXEDIR\installed-test"
!else
  !define PRODUCT "NIP Native"
  !define APP_EXE "NIPNative.exe"
  !define REGKEY "Software\NIPNative"
  !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\NIPNative"
  InstallDir "$LOCALAPPDATA\Programs\NIPNative"
!endif

Name "${PRODUCT}"
OutFile "${OUTPUT}"
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor /SOLID lzma
InstallDirRegKey HKCU "${REGKEY}" "InstallLocation"
VIProductVersion "0.2.0.0"
VIAddVersionKey "ProductName" "NIP"
VIAddVersionKey "FileDescription" "Instalador de NIP Native"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 Santiago Abregu"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_INSTFILES
Page custom FinishCreate FinishLeave
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Spanish"

Var DesktopCheckbox
Var StartCheckbox
Var RunCheckbox

Section "NIP"
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File /oname=${APP_EXE} "${BINARY}"
  File /oname=LICENSE "..\LICENSE"
  File /oname=THIRD_PARTY_NOTICES.txt "${NOTICES}"
  WriteUninstaller "$INSTDIR\Uninstall NIP Native.exe"
  WriteRegStr HKCU "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Santiago Abregu"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall NIP Native.exe$\"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Function FinishCreate
  !insertmacro MUI_HEADER_TEXT "NIP se instaló correctamente" "Elegí cómo querés empezar."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    MessageBox MB_OK "NIP se instaló. Podés abrirlo desde $INSTDIR."
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 26u "La instalación terminó. Seleccioná cualquier combinación y pulsá Finalizar."
  Pop $0
  ${NSD_CreateCheckbox} 0 38u 100% 16u "Crear acceso directo en el escritorio"
  Pop $DesktopCheckbox
  ${NSD_Check} $DesktopCheckbox
  ${NSD_CreateCheckbox} 0 60u 100% 16u "Crear acceso en el menú Inicio"
  Pop $StartCheckbox
  ${NSD_Check} $StartCheckbox
  ${NSD_CreateCheckbox} 0 82u 100% 16u "Abrir NIP ahora"
  Pop $RunCheckbox
  ${NSD_Check} $RunCheckbox
  ${NSD_CreateLabel} 0 108u 100% 24u "El acceso en Inicio no ancla la app. Podés anclarla manualmente desde Windows."
  Pop $0
  SendMessage $mui.Button.Next ${WM_SETTEXT} 0 "STR:Finalizar"
  GetDlgItem $0 $HWNDPARENT 3
  EnableWindow $0 0
  GetDlgItem $0 $HWNDPARENT 2
  ShowWindow $0 ${SW_HIDE}
  nsDialogs::Show
FunctionEnd

Function FinishLeave
  SetShellVarContext current
  ${NSD_GetState} $DesktopCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    ClearErrors
    CreateShortCut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${APP_EXE}"
    ${If} ${Errors}
      MessageBox MB_OK "No se pudo crear el acceso en el escritorio. Reintentá o desmarcá la opción."
      Abort
    ${EndIf}
  ${EndIf}
  ${NSD_GetState} $StartCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    ClearErrors
    CreateShortCut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${APP_EXE}"
    ${If} ${Errors}
      MessageBox MB_OK "No se pudo crear el acceso en Inicio. Reintentá o desmarcá la opción."
      Abort
    ${EndIf}
  ${EndIf}
  ${NSD_GetState} $RunCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    ClearErrors
    !ifdef TEST_BUILD
      ExecShell "open" "$INSTDIR\${APP_EXE}" '--data-dir $\"$EXEDIR\profile-test$\"'
    !else
      ExecShell "open" "$INSTDIR\${APP_EXE}"
    !endif
    ${If} ${Errors}
      MessageBox MB_OK "NIP se instaló, pero Windows no pudo abrirlo. El ejecutable sigue en $INSTDIR."
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Uninstall"
  SetShellVarContext current
  ClearErrors
  Delete "$INSTDIR\${APP_EXE}"
  ${If} ${Errors}
    MessageBox MB_OK "Cerrá ${PRODUCT} antes de desinstalar. Tus datos se conservan."
    Abort
  ${EndIf}
  Delete "$DESKTOP\${PRODUCT}.lnk"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.txt"
  Delete "$INSTDIR\Uninstall NIP Native.exe"
  ; Only known distribution files are removed. Never recursively delete a user's directory.
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${REGKEY}"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
