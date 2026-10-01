; Registers Lumen Reader as the handler for .epub files.
;
; Tauri calls these hooks from its generated NSIS script:
;   customInit   - before the UI appears
;   postInstall  - after files are laid down
;   customUnInstall - during removal
;
; Writes are HKCU-only (this installer is a per-user install), so no elevation
; is required. Windows 10+ still honours a UserChoice override if the user has
; already chosen a different app in "Default apps"; otherwise pointing
; HKCR\.epub at our ProgID makes us the default immediately.

!macro customInit
  !insertmacro WEBVIEW2_CHECK_HOOK
!macroend

!macro postInstall
  WriteRegStr HKCU "Software\Classes\.epub" "" "LumenReader.EPUB"
  WriteRegStr HKCU "Software\Classes\.epub" "PerceivedType" "text"

  WriteRegStr HKCU "Software\Classes\LumenReader.EPUB" "" "EPUB document"
  WriteRegStr HKCU "Software\Classes\LumenReader.EPUB\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr HKCU "Software\Classes\LumenReader.EPUB\shell" "" "open"
  WriteRegStr HKCU "Software\Classes\LumenReader.EPUB\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\LumenReader.EPUB\CurVer" "" "LumenReader.EPUB"

  ; Makes the app show up in "Open with" for every .epub on the machine.
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe" "" "EPUB reader"
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\supportedTypes" ".epub" ""

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
!macroend

!macro customUnInstall
  DeleteRegKey HKCU "Software\Classes\LumenReader.EPUB"
  DeleteRegValue HKCU "Software\Classes\.epub" ""
  DeleteRegValue HKCU "Software\Classes\.epub" "PerceivedType"
  DeleteRegKey HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
!macroend