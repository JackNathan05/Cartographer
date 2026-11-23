; Cartographer installer hooks (Tauri NSIS).

!macro NSIS_HOOK_PREUNINSTALL
  ; close the tray app so its files can be removed
  nsExec::Exec 'taskkill /IM cartographer.exe /F'
  ; remove "Start with Windows"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Cartographer"
  ; fast scan helper (Program Files + a scheduled task) needs admin to remove: one Windows prompt
  IfFileExists "$PROGRAMFILES64\Cartographer\cartographer-helper.exe" 0 +2
    ExecShellWait "runas" "$INSTDIR\cartographer.exe" "--uninstall-helper"
!macroend
