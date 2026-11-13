!include nsDialogs.nsh

!ifndef BUILD_UNINSTALLER
Var NipDesktopCheckbox
Var NipStartCheckbox
Var NipRunCheckbox



!macro customInstall
  ${If} ${Silent}
    Call NipCreateDesktopShortcut
    Call NipCreateStartShortcut
  ${EndIf}
!macroend

!macro customFinishPage
  Page custom NipFinishCreate NipFinishLeave
!macroend


!macro customHeader
Function NipCreateDesktopShortcut
  ClearErrors
  CreateShortCut "$DESKTOP\${SHORTCUT_NAME}.lnk" "$INSTDIR\${APP_EXECUTABLE_FILENAME}"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONEXCLAMATION "No se pudo crear el acceso en el escritorio. Podés reintentar o desmarcar esta opción."
    Abort
  ${EndIf}
  WinShell::SetLnkAUMI "$DESKTOP\${SHORTCUT_NAME}.lnk" "${APP_ID}"
FunctionEnd
Function NipCreateStartShortcut
  ClearErrors
  CreateShortCut "$SMPROGRAMS\${SHORTCUT_NAME}.lnk" "$INSTDIR\${APP_EXECUTABLE_FILENAME}"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONEXCLAMATION "No se pudo crear el acceso en el menú Inicio. Podés reintentar o desmarcar esta opción."
    Abort
  ${EndIf}
  WinShell::SetLnkAUMI "$SMPROGRAMS\${SHORTCUT_NAME}.lnk" "${APP_ID}"
FunctionEnd
Function NipFinishCreate
  !insertmacro MUI_HEADER_TEXT "NIP se instaló correctamente" "Elegí cómo querés empezar."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    MessageBox MB_OK|MB_ICONEXCLAMATION "NIP se instaló, pero no se pudo mostrar la pantalla final. Podés abrirlo desde su carpeta de instalación."
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 26u "La instalación terminó. Seleccioná las opciones que quieras y pulsá Finalizar."
  Pop $0
  ${NSD_CreateCheckbox} 0 38u 100% 16u "Crear acceso directo en el escritorio"
  Pop $NipDesktopCheckbox
  ${NSD_Check} $NipDesktopCheckbox
  ${NSD_CreateCheckbox} 0 60u 100% 16u "Crear acceso en el menú Inicio"
  Pop $NipStartCheckbox
  ${NSD_Check} $NipStartCheckbox
  ${NSD_CreateCheckbox} 0 82u 100% 16u "Abrir NIP ahora"
  Pop $NipRunCheckbox
  ${NSD_Check} $NipRunCheckbox
  ${NSD_CreateLabel} 0 108u 100% 24u "El acceso en Inicio no ancla la app. Podés anclarla manualmente desde Windows."
  Pop $0
  GetDlgItem $0 $HWNDPARENT 1
  ${NSD_SetText} $0 "Finalizar"
  GetDlgItem $0 $HWNDPARENT 3
  EnableWindow $0 0
  GetDlgItem $0 $HWNDPARENT 2
  ShowWindow $0 ${SW_HIDE}
  nsDialogs::Show
FunctionEnd
Function NipFinishLeave
  ${NSD_GetState} $NipDesktopCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    Call NipCreateDesktopShortcut
  ${EndIf}
  ${NSD_GetState} $NipStartCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    Call NipCreateStartShortcut
  ${EndIf}
  ${NSD_GetState} $NipRunCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    ExecShell "open" "$INSTDIR\${APP_EXECUTABLE_FILENAME}"
    ${If} ${Errors}
      MessageBox MB_OK|MB_ICONEXCLAMATION "NIP se instaló, pero no se pudo abrir. Podés abrirlo desde su carpeta o acceso directo."
    ${EndIf}
  ${EndIf}
FunctionEnd
!macroend
!endif

!macro customUnInstall
  ${IfNot} ${isKeepShortcuts}
    WinShell::UninstShortcut "$oldDesktopLink"
    Delete "$oldDesktopLink"
    WinShell::UninstShortcut "$oldStartMenuLink"
    Delete "$oldStartMenuLink"
  ${EndIf}
!macroend
