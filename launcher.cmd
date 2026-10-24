@echo off
rem The remaster's launcher (crates\qwc-launcher), in a release build, with the game it starts built beside it.
rem Its library (the imported game files, the packs, the profile) is %APPDATA%\QWCRemastered.
rem What it needs installed is in README.md, "Building from source".
setlocal
cd /d "%~dp0"

rem NASM builds OpenH264's assembly on the first build (winget puts it here)
if exist "%LOCALAPPDATA%\bin\NASM\nasm.exe" set "PATH=%PATH%;%LOCALAPPDATA%\bin\NASM"

cargo build --release -p qwc-game -p qwc-launcher -p qwc-studio || (pause & exit /b 1)
start "" "target\release\qwc-launcher.exe"
endlocal
