; Tauri packages up to 0.0.93 used the Cargo bin name (ModeTauri.exe) as the
; installed executable. Since mainBinaryName is now "Mode", the new file list no
; longer includes the legacy binary; remove it so an in-place upgrade does not
; leave two main executables in the same install directory.
!macro NSIS_HOOK_POSTINSTALL
  Delete "$INSTDIR\ModeTauri.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$INSTDIR\ModeTauri.exe"
!macroend
