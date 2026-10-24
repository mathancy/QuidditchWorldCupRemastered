"""Stage a release: the remaster's programs and the files it ships, laid out as the programs look for them.

    python packaging/stage.py --bin target/release --out STAGE [--notices FILE]

The programs find the shipped files from their own folder (`qwc-launcher`'s `resources_root`: the folder with
`remastered/fonts.txt`), and the game and the Studio read them relative to it, so the stage keeps the repo's paths:

    qwc-launcher(.exe)  qwc-game(.exe)  qwc-studio(.exe)
    remastered/fonts.txt, remastered/fonts/      the vector fonts and their licences (SIL OFL), the launcher's
    crates/qwc-launcher/art/                      the launcher's pictures (docs/LAUNCHER_ART.md)
    LICENSE, THIRD-PARTY-LICENSES.html (--notices)

The game itself ships no remastered content: without a pack on it plays as the original (its stock HUD is built
into the program); screen layouts, HUD scenes, looks and the game's vector fonts come in resource packs.

Only files Git tracks are staged (a working tree's local edits and untracked files stay out), and the stage is
refused if a font is still a Git LFS pointer. Nothing of the game's is ever staged: the release is code and the
remaster's own files (docs/LAUNCHER.md, "Release").
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

PROGRAMS = ["qwc-launcher", "qwc-game", "qwc-studio"]

SHIPPED = [
    "remastered/fonts.txt",
    "remastered/fonts",
    "crates/qwc-launcher/art",
    "LICENSE",
]

# what the shipped folders may hold: anything else is refused, so nothing unexpected rides along
ALLOWED = {".ttf", ".otf", ".txt", ".png", ""}


def tracked(paths):
    out = subprocess.run(["git", "ls-files", "-z", "--", *paths], cwd=ROOT, check=True, capture_output=True).stdout
    return [p for p in out.decode("utf-8").split("\0") if p]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--bin", required=True, help="the folder with the built programs (target/release)")
    ap.add_argument("--out", required=True, help="the stage folder (emptied first)")
    ap.add_argument("--notices", help="the third-party licences page to ship as THIRD-PARTY-LICENSES.html")
    a = ap.parse_args()

    out = Path(a.out)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    exe = ".exe" if sys.platform == "win32" else ""
    for name in PROGRAMS:
        src = Path(a.bin) / (name + exe)
        if not src.is_file():
            sys.exit(f"{src}: not built")
        shutil.copy2(src, out / src.name)

    files = tracked(SHIPPED)
    for rel in files:
        src = ROOT / rel
        if Path(rel).suffix.lower() not in ALLOWED:
            sys.exit(f"{rel}: not a kind of file the release ships")
        if src.stat().st_size < 1024 and src.read_bytes().startswith(b"version https://git-lfs"):
            sys.exit(f"{rel}: a Git LFS pointer (run `git lfs pull`)")
        dst = out / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
    for must in ["remastered/fonts.txt", "LICENSE"]:
        if not (out / must).is_file():
            sys.exit(f"{must}: missing from the stage")
    if a.notices:
        shutil.copy2(a.notices, out / "THIRD-PARTY-LICENSES.html")
    print(f"staged {len(PROGRAMS)} programs and {len(files)} files in {out}")


if __name__ == "__main__":
    main()
