@echo off
rem The HUD editor on the match's HUD, in a release build.
rem Extra arguments go to `qwc-studio`, for example:
rem   hud-editor.cmd                  the match's HUD (saved to the highest pack's hud\match.ron)
rem   hud-editor.cmd challenge        a challenge's HUD
rem   hud-editor.cmd --screen StartScreen   the front end's screens, from the title (the highest pack's screens\)
rem Without a pack on, the stock HUD and screens show and nothing is saved.
rem What it needs installed is in README.md, "Building from source".
setlocal
cd /d "%~dp0"
cargo run --release -p qwc-studio -- %*
if errorlevel 1 pause
endlocal
