#!/bin/sh
# QWC Studio (crates/qwc-studio) on its Packs mode, in a release build, with the game it plays packs in beside it.
# Extra arguments go to `qwc-studio`, for example `./studio.sh --open ~/my.pack`.
# hud-editor.sh opens the same Studio on the HUD. What it needs installed is in README.md, "Building from source".
set -e
cd "$(dirname "$0")"
cargo build --release -p qwc-game -p qwc-studio
exec target/release/qwc-studio --packs "$@"
