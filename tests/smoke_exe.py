"""Smoke test for the built dubplate.exe (run by the Windows build):
    python tests/smoke_exe.py dist/dubplate.exe

Makes a fake library (including a path longer than Windows' 260-character limit),
then runs the real .exe on it: find, copy, and copy again (must skip what's already there).
Then a rekordbox stick from two real WAV files, a second set on it, and its backups.
"""
import os
import struct
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
    print(f"$ dubplate {' '.join(args)}\n{r.stdout}{r.stderr}")
    return r


r = run("--version")
assert r.returncode == 0 and "dubplate" in r.stdout, "the exe doesn't start"
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

# ---- a rekordbox stick (analysis, databases, backups) from real audio
def click_wav(path, bpm, seconds=12, rate=44100):
    beat = int(60 / bpm * rate)
    frames = bytearray()
    for i in range(rate * seconds):
        v = (20000 if (i // 20) % 2 == 0 else -20000) if i % beat < 400 else 0
        frames += struct.pack("<h", v)
    with open(path, "wb") as fh:
        fh.write(b"RIFF" + struct.pack("<I", 36 + len(frames)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
                 + b"data" + struct.pack("<I", len(frames)) + frames)


rblib = os.path.join(tmp, "RbMusic", "Tester")
os.makedirs(rblib)
for title, bpm in (("Steady", 120), ("Faster", 128)):
    click_wav(os.path.join(rblib, f"Tester - {title}.wav"), bpm)
rblist = os.path.join(tmp, "Friday.txt")
with open(rblist, "w", encoding="utf-8") as fh:
    fh.write("Tester - Steady\nTester - Faster\n")
stick = os.path.join(tmp, "stick")
os.makedirs(stick)
music2 = os.path.join(tmp, "RbMusic")
r = run("copy", rblist, stick, "--rekordbox", "--music", music2)
assert r.returncode == 0 and "Done: 2 songs in the playlist Friday" in r.stdout, "rekordbox export failed"
assert os.path.isfile(os.path.join(stick, "PIONEER", "rekordbox", "export.pdb"))
assert os.path.isfile(os.path.join(stick, "PIONEER", "rekordbox", "exportLibrary.db"))
r = run("copy", rblist, stick, "--rekordbox", "--name", "Saturday", "--music", music2)
assert r.returncode == 0 and "2 already on the stick" in r.stdout and "backed up" in r.stdout, "second set failed"
r = run("restore-stick", stick)
assert r.returncode == 0 and "Backups of" in r.stdout, "no backups listed"
backup_name = r.stdout.splitlines()[1].strip()
r = run("restore-stick", stick, backup_name)
assert r.returncode == 0 and "Put back" in r.stdout, "restore failed"
for f in os.listdir(rblib):  # the library is untouched
    assert os.path.getsize(os.path.join(rblib, f)) > 1000
print("SMOKE TEST PASSED")
