@echo off
rem QWC Studio (crates\qwc-studio) on its Packs mode, in a release build, with the game it plays packs in beside it.
rem Extra arguments go to `qwc-studio`, for example:
rem   studio.cmd --open C:\path\to\my.pack     that pack folder
rem hud-editor.cmd opens the same Studio on the HUD. What it needs installed is in README.md, "Building from source".
setlocal
cd /d "%~dp0"

rem NASM builds OpenH264's assembly on the first build (winget puts it here)
if exist "%LOCALAPPDATA%\bin\NASM\nasm.exe" set "PATH=%PATH%;%LOCALAPPDATA%\bin\NASM"

cargo build --release -p qwc-game -p qwc-studio || (pause & exit /b 1)
target\release\qwc-studio.exe --packs %*
if errorlevel 1 pause
endlocal
