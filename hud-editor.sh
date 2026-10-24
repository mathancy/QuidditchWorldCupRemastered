#!/usr/bin/env bash
# The HUD editor on the match's HUD, in a release build.
# Extra arguments go to `qwc-studio`, for example:
#   ./hud-editor.sh                 the match's HUD (saved to the highest pack's hud/match.ron)
#   ./hud-editor.sh challenge       a challenge's HUD
#   ./hud-editor.sh --screen StartScreen   the front end's screens, from the title (the highest pack's screens/)
# Without a pack on, the stock HUD and screens show and nothing is saved.
# What it needs installed is in README.md, "Building from source".
set -euo pipefail
cd "$(dirname "$0")"

exec cargo run --release -p qwc-studio -- "$@"
