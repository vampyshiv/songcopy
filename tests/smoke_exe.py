"""Smoke test for the built songcopy.exe (run by the Windows build):
    python tests/smoke_exe.py dist/songcopy.exe

Makes a fake library (including a path longer than Windows' 260-character limit),
then runs the real .exe on it: find, copy, and copy again (must skip what's already there).
"""
import os
import subprocess
import sys
import tempfile

exe = os.path.abspath(sys.argv[1])
tmp = tempfile.mkdtemp()
lib = os.path.join(tmp, "Music")
LONG_ALBUM = "A Very Long Album Name That Goes On And On " * 3 + "(2020)"

songs = {
    "Robyn - Dancing On My Own": f"Robyn/Body Talk (2010)/Robyn - Body Talk - 03 - Dancing on My Own.flac",
    "Emptyset - Avichi": f"Emptyset/Avichi EP (2011)/Emptyset - Avichi EP - 01 - Avichi (original mix).flac",
    "Long Artist - Long Song": f"Long Artist/{LONG_ALBUM}/Long Artist - {LONG_ALBUM[:-7]} - 01 - Long Song.flac",
}


def winlong(p):
    p = os.path.abspath(p)
    return "\\\\?\\" + p if os.name == "nt" else p


for rel in songs.values():
    p = os.path.join(lib, *rel.split("/"))
    os.makedirs(winlong(os.path.dirname(p)), exist_ok=True)
    with open(winlong(p), "wb") as fh:
        fh.write(rel.encode() * 100)

longest = max(len(os.path.join(lib, *r.split("/"))) for r in songs.values())
print(f"longest library path: {longest} characters")
assert os.name != "nt" or longest > 260, "the long-path case isn't long enough to test anything"

lst = os.path.join(tmp, "list.txt")
with open(lst, "w", encoding="utf-8") as fh:
    fh.write("\n".join(songs) + "\n")
dest = os.path.join(tmp, "Traktor")


def run(*args):
    r = subprocess.run([exe, *args], capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=300,
                       env={**os.environ, "LOCALAPPDATA": os.path.join(tmp, "appdata")})
    print(f"$ songcopy {' '.join(args)}\n{r.stdout}{r.stderr}")
    return r


r = run("--version")
assert r.returncode == 0 and "songcopy" in r.stdout, "the exe doesn't start"
r = run("find", lst, "--music", lib)
assert r.returncode == 0 and "Found 3 exact" in r.stdout, "find failed"
r = run("copy", lst, dest, "--music", lib)
assert r.returncode == 0, "copy failed"
copied = sorted(os.listdir(dest))
assert len(copied) == 3, copied
r = run("copy", lst, dest, "--music", lib)
assert "3 already copied" in r.stdout and "Nothing new to copy" in r.stdout, "memory failed"
for rel in songs.values():   # the library is untouched
    assert os.path.exists(winlong(os.path.join(lib, *rel.split("/"))))
print("SMOKE TEST PASSED")
