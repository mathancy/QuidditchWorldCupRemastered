#!/usr/bin/env bash
# The front end from the boot, with the remastered movies (remastered/movies), in a release build.
# Extra arguments go to `qwc-game frontend`, for example:
#   ./frontend.sh --size 1920x1080
#   ./frontend.sh --original-movies     the disc's movies instead
#   ./frontend.sh --openh264            the remastered movies through OpenH264 on Windows too
# What it needs installed is in README.md, "Building from source".
set -euo pipefail
cd "$(dirname "$0")"

# the remastered movies are your own (not in the repo): without them the disc's play
if [ ! -f remastered/movies/Qintro.mp4 ] || [ "$(wc -c < remastered/movies/Qintro.mp4)" -lt 1024 ]; then
    echo "No remastered movies in remastered/movies: the disc's movies will play."
fi

exec cargo run --release -p qwc-game -- frontend boot "$@"
