@echo off
rem The front end from the boot, with the remastered movies (remastered\movies), in a release build.
rem Extra arguments go to `qwc-game frontend`, for example:
rem   frontend.cmd --size 1920x1080
rem   frontend.cmd --original-movies      the disc's movies instead
rem   frontend.cmd --openh264             the remastered movies through OpenH264 rather than Media Foundation
rem What it needs installed is in README.md, "Building from source".
setlocal
cd /d "%~dp0"

rem NASM builds OpenH264's assembly on the first build (winget puts it here)
if exist "%LOCALAPPDATA%\bin\NASM\nasm.exe" set "PATH=%PATH%;%LOCALAPPDATA%\bin\NASM"

rem the remastered movies are your own (not in the repo): without them the disc's play
set "HD=1"
if not exist remastered\movies\Qintro.mp4 set "HD="
if defined HD for %%F in (remastered\movies\Qintro.mp4) do if %%~zF LSS 1024 set "HD="
if not defined HD echo No remastered movies in remastered\movies: the disc's movies will play.

cargo run --release -p qwc-game -- frontend boot %*
if errorlevel 1 pause
endlocal
