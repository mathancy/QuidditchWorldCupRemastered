#!/bin/sh
# The remaster's launcher (crates/qwc-launcher), in a release build, with the game it starts built beside it.
# Its library (the imported game files, the packs, the profile) is ~/.local/share/qwc-remastered.
# What it needs installed is in README.md, "Building from source".
set -e
cd "$(dirname "$0")"
cargo build --release -p qwc-game -p qwc-launcher -p qwc-studio
exec target/release/qwc-launcher "$@"
