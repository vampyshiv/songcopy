#!/usr/bin/env python3
# songcopy - copy a list of songs out of a music library
# Copyright (C) 2026 vampyshiv
# SPDX-License-Identifier: AGPL-3.0-or-later
# This program is free software under the GNU Affero General Public License v3 or later; see LICENSE.
# =============================================================================
# songcopy: copy a list of songs out of ~/Music into another folder
# =============================================================================
#   songcopy                          start + open the web page in your browser
#   songcopy serve [--port N]         same thing, explicitly (--no-browser, --dest FOLDER)
#   songcopy copy LIST DEST           copy every song in LIST into folder DEST
#   songcopy find LIST                just check which songs can be found (copies nothing)
#
# FROM A WINDOWS PC (e.g. straight into your Traktor folder): double-click songcopy.exe
#   (from the GitHub releases). It finds this server on whatever network you're on, signs in
#   to the share once and opens the page. Copies go to folders on the Windows PC.
#   Moved house / new network?  Run `stacks set-lan-ip` here; the .exe finds the new address.
#
# LIST can be: .csv / .tsv / .xlsx spreadsheet, .txt (one song per line),
#              or an .m3u / .m3u8 playlist.
#   Spreadsheet columns it understands (any order, case doesn't matter):
#     Artist | Title (or Song / Track / Name) | Album | File (or Path / Filename)
#   No header row? Each line is read as "Artist - Title", a filename, or a path.
#
# Options for `copy`:
#   --prefer flac|mp3|any   if a song exists in several formats, which one (default flac)
#   --folders               keep Artist/Album folders (default: all files in one folder)
#   --number                put list order in front of names: "001 - ..."
#   --playlist              also write a playlist.m3u8 into DEST
#   --strict                only copy exact matches (skip "best guess" ones)
#   --any-version           asked for e.g. an Extended Mix you don't have? copy the version you DO have
#                           (default: don't copy it, list it in the missing file instead)
#   --usb                   make a USB for CDJs/XDJs: DEST/<list name>/001 - ..., and anything the decks
#                           can't play (FLAC, Opus, ALAC, hi-res) is converted (needs ffmpeg)
#   --usb-format aiff|mp3   what --usb converts to (default aiff = lossless)
#   --name NAME             playlist folder name for --usb
#   --recopy                copy again songs already copied to DEST on an earlier run
#                           (normally skipped, so re-running a list only copies the new finds)
#   --dry-run               show what would happen, copy nothing
#   --music PATH            music library (default ~/Music)
#
# SAFE BY DESIGN: it only ever COPIES. It never moves, deletes or overwrites a file.
# A file already in DEST with the same name and size is skipped; a different file
# with the same name gets " (2)" added instead of being replaced.
#
# Songs it can't find are printed in red and saved to "<LIST>-missing.csv".
#
# Tip: on your PATH as `songcopy` via  ~/.local/bin/songcopy → this file (~/songcopy/songcopy.py)
# =============================================================================
import argparse
import functools
import base64
import csv
import io
import json
import os
import re
import shutil
import sys
import threading
import time
import unicodedata
import uuid
import zipfile
import xml.etree.ElementTree as ET
from collections import Counter
from datetime import datetime
from difflib import SequenceMatcher
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

AUDIO_EXT = {".flac", ".mp3", ".m4a", ".opus", ".ogg", ".wav", ".aiff", ".aif", ".wma", ".alac", ".ape", ".wv"}
VERSION = "1.1.0"
DEFAULT_MUSIC = os.path.expanduser("~/Music")
SKIP_DIRS = {"Podcasts"}  # top-level folders that aren't songs
WINDOWS = os.name == "nt"

def longp(p):
    """Windows only: allow paths longer than 260 characters (123 songs in this library need it)."""
    if not WINDOWS or not p or p.startswith("\\\\?\\"):
        return p
    p = os.path.abspath(p)
    return "\\\\?\\UNC\\" + p[2:] if p.startswith("\\\\") else "\\\\?\\" + p

def isfile(p):
    return os.path.isfile(longp(p))

def getsize(p):
    return os.path.getsize(longp(p))

# ----------------------------------------------------------------------------- text helpers

def norm(s):
    """lowercase, no accents, no punctuation: "Don't Stop (Björk Mix)" -> "dont stop bjork mix" """
    s = "".join(c for c in unicodedata.normalize("NFKD", str(s)) if not unicodedata.combining(c)).lower()
    s = s.replace("ß", "ss").replace("ø", "o").replace("æ", "ae").replace("đ", "d").replace("ł", "l")
    s = s.replace("&", " and ").replace("'", "").replace("’", "").replace("‘", "")
    s = re.sub(r"[\W_]+", " ", s)   # keeps letters of every alphabet (日本語, кириллица…)
    return s.strip()

def tokens(s):
    return [t for t in norm(s).split() if t]

NOISE = re.compile(
    r"\s*[\(\[][^\)\]]*(remaster|remastered|deluxe|bonus|explicit|clean|edit|version|mono|stereo|live|feat|ft\.|with )[^\)\]]*[\)\]]"
    r"|\s+-\s+.*(remaster|version|edit|mix|live|mono|stereo).*$"
    r"|\s+(feat\.?|ft\.?|featuring)\s.*$",
    re.I,
)

# words in a "(...)" / " - ..." version tag that DON'T make it a different recording:
# "(Original Mix)", "(Album Version)", "- Remastered 2011" are all just the song itself.
HARMLESS = {"remaster", "remastered", "remasters", "explicit", "clean", "deluxe", "bonus", "track", "edition",
            "mono", "stereo", "digital", "digitally", "single", "album", "version", "lp", "original", "the", "and",
            "mix", "dirty", "uncensored", "censored", "anniversary", "expanded", "official", "audio", "hq", "hd"}

VERSION_TAIL = re.compile(r"\s*(?:[\(\[]([^\(\)\[\]]*)[\)\]]|\s[-–—]\s+([^-–—\(\)\[\]]*))\s*$")

@functools.lru_cache(maxsize=65536)
def split_version(t):
    """'In Violet (Salem Remix)' -> ('In Violet', {'salem','remix'});  'Avichi (Original Mix)' -> ('Avichi', {})"""
    t, tags = t or "", []
    while True:
        m = VERSION_TAIL.search(t)
        if not m or m.start() == 0:
            break
        tags.append(m.group(1) if m.group(1) is not None else m.group(2))
        t = t[:m.start()]
    words = []
    for tag in reversed(tags):
        if re.match(r"\s*(feat\.?|ft\.?|featuring|with|prod\.?)\s", tag or "", re.I):
            continue   # "(feat. X)" is not a different version
        words += [w for w in tokens(tag) if not harmless(w)]
    t = re.sub(r"\s+(feat\.?|ft\.?|featuring)\s.*$", "", t, flags=re.I)
    return t.strip(), "".join(words)   # e.g. "salemremix" - spaces ignored, so "Nobu bros" = "Nobubros"

@functools.lru_cache(maxsize=8192)
def harmless(w):
    if w in HARMLESS or w.isdigit():
        return True
    from difflib import get_close_matches   # "Rmeastered" is still "Remastered"
    return len(w) >= 6 and bool(get_close_matches(w, list(HARMLESS), 1, 0.85))

def same_version(a, b):
    if a == b:
        return True
    if not (a and b):
        return False
    # "RAC mix" typed as "RCA mix": same letters, swapped
    return SequenceMatcher(None, a, b).ratio() >= 0.85 or (len(a) >= 3 and sorted(a) == sorted(b))

def clean_title(t):
    """'Song - Remastered 2011' / 'Song (feat. X)' -> 'Song'"""
    c = NOISE.sub("", t or "").strip()
    return c or t

def first_artist(a):
    return re.split(r"\s*[;,/]\s*|\s+(?:feat\.?|ft\.?|featuring|x|vs\.?)\s+", a or "", flags=re.I)[0].strip()

# ----------------------------------------------------------------------------- the library index

FNAME_RE = re.compile(r"^(?P<artist>.+?) - (?P<album>.+) - (?P<track>\d{1,3}) - (?P<title>.+)$")

class Library:
    def __init__(self, root):
        self.root = os.path.abspath(os.path.expanduser(root))
        self.files = []        # list of dicts
        self.by_name = {}      # lowercase basename / stem -> [idx]
        self.index = {}        # token -> set(idx)
        self.scanned_at = None
        self.lock = threading.Lock()
        self.ready = threading.Event()   # set once there is something to search
        self.scanning = False
        self.progress = 0

    def _walk(self):
        """yield (full path, rel path, size) for every audio file. Uses scandir so that over a
        Windows network share the size comes free with the directory listing."""
        base = longp(self.root)
        stack = [(base, "")]
        while stack:
            d, rel = stack.pop()
            try:
                with os.scandir(d) as it:
                    entries = sorted(it, key=lambda e: e.name.lower())
            except OSError:
                continue
            subdirs = []
            for e in entries:
                try:
                    if e.is_dir(follow_symlinks=True):
                        if not (rel == "" and e.name in SKIP_DIRS) and not e.name.startswith("."):
                            subdirs.append((e.path, os.path.join(rel, e.name) if rel else e.name))
                    elif os.path.splitext(e.name)[1].lower() in AUDIO_EXT:
                        r = os.path.join(rel, e.name) if rel else e.name
                        yield os.path.join(self.root, r), r, e.stat().st_size
                except OSError:
                    continue
            stack.extend(reversed(subdirs))

    def scan(self, quiet=True):
        self.scanning, self.progress = True, 0
        files = []
        try:
            for full, rel, size in self._walk():
                fn = os.path.basename(rel)
                stem, ext = os.path.splitext(fn)
                parts = rel.split(os.sep)
                m = FNAME_RE.match(stem)
                if m:
                    artist, album, title = m["artist"], m["album"], m["title"]
                else:
                    # "Artist - Title" loose file, or something else
                    bits = stem.split(" - ")
                    title = re.sub(r"^\d{1,3}[\s.\-_]+", "", bits[-1]) if bits else stem
                    artist = bits[0] if len(bits) > 1 else (parts[-3] if len(parts) >= 3 else "")
                    album = parts[-2] if len(parts) >= 3 else ""
                    album = re.sub(r"\s*\(\d{4}\)$", "", album)
                files.append({"path": full, "rel": rel, "name": fn, "ext": ext.lower().lstrip("."),
                              "artist": artist, "album": album, "title": title, "size": size,
                              "n_dirartist": norm(parts[-3]) if len(parts) >= 3 else ""})
                self.progress = len(files)
                if not quiet and C.on and len(files) % 2000 == 0:
                    print(f"\r{C.D}Reading your library ... {len(files):,} songs{C.X}", end="", flush=True)
            self._install(files)
            self._save_cache()
        finally:
            self.scanning = False
        return len(self.files)

    def _install(self, files, when=None):
        by_name, index = {}, {}
        for i, f in enumerate(files):
            f["n_artist"], f["n_title"], f["n_album"] = norm(f["artist"]), norm(f["title"]), norm(f["album"])
            b, f["vtags"] = split_version(f["title"])
            f["n_base"] = norm(b)
            stem = os.path.splitext(f["name"])[0]
            by_name.setdefault(f["name"].lower(), []).append(i)
            by_name.setdefault(stem.lower(), []).append(i)
            for t in set(tokens(f["rel"].rsplit(".", 1)[0].replace(os.sep, " "))):
                index.setdefault(t, set()).add(i)
        with self.lock:
            self.files, self.by_name, self.index = files, by_name, index
            self.scanned_at = when or datetime.now()
        self.ready.set()

    # A saved copy of the song list, so the page opens instantly next time (matters on Windows,
    # where reading 25,000 files over the network takes a while). Always refreshed in the background.
    def _cache_file(self):
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~/.cache")
        import hashlib
        return os.path.join(base, "songcopy", hashlib.sha1(self.root.encode()).hexdigest()[:12] + ".json")

    def _save_cache(self):
        keep = ("path", "rel", "name", "ext", "artist", "album", "title", "size", "n_dirartist")
        try:
            p = self._cache_file()
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p + ".tmp", "w", encoding="utf-8") as fh:
                json.dump({"root": self.root, "when": self.scanned_at.isoformat(),
                           "files": [{k: f[k] for k in keep} for f in self.files]}, fh)
            os.replace(p + ".tmp", p)
        except OSError:
            pass

    def load_cache(self):
        try:
            with open(self._cache_file(), encoding="utf-8") as fh:
                d = json.load(fh)
            if d.get("root") == self.root and d.get("files"):
                self._install(d["files"], datetime.fromisoformat(d["when"]))
                return True
        except (OSError, ValueError, KeyError):
            pass
        return False

    # ------------------------------------------------------------------ matching

    def search(self, q, prefer="flac"):
        """the manual search box: every file whose path contains all the words, best first"""
        idxs = self._candidates(tokens(q))
        if idxs:
            out = [dict(pub(f), score=round(s, 3)) for s, f in self._rank(idxs, "", q, "", prefer)]
        else:
            r = self.match({"raw": q}, prefer)
            out = ([r["file"]] + r["alternatives"]) if r["file"] else r["suggestions"]
        return out[:30]

    def _candidates(self, toks):
        """files whose path contains ALL of these words"""
        toks = [t for t in dict.fromkeys(toks) if t]
        if not toks:
            return set()
        posting = []
        for t in toks:
            p = self.index.get(t)
            if p is None:
                return set()
            posting.append(p)
        posting.sort(key=len)
        out = set(posting[0])
        for p in posting[1:]:
            out &= p
            if not out:
                break
        return out

    def _score(self, f, artist, title, album):
        """0..1, how well file f matches what was asked for"""
        nt, na = norm(title), norm(artist)
        if not nt:
            s_title = 0.5
        elif nt == f["n_title"]:
            s_title = 1.0
        else:
            s_title = SequenceMatcher(None, nt, f["n_title"]).ratio()
            if self._same_song(title, f):
                rt = split_version(title)[1]
                if rt == f["vtags"]:
                    # same recording: "Avichi" = "Avichi (Original Mix)", "- Remastered 2011"
                    s_title = max(s_title, 0.97 if norm(split_version(title)[0]) == f["n_base"] else 0.93)
                elif same_version(rt, f["vtags"]):
                    s_title = max(s_title, 0.88)      # typo in the version name
                else:
                    # same song, different version (asked for Extended Mix, have Radio Edit...)
                    # the plain original first, then the most similar other version
                    s_title = 0.86 if not f["vtags"] else 0.8 + 0.05 * s_title
        if na:
            fa = f["n_artist"]
            if na == fa or na == f["n_dirartist"]:
                s_art = 1.0
            elif na in fa or fa in na or na in f["n_dirartist"]:
                s_art = 0.9
            else:
                s_art = SequenceMatcher(None, na, fa).ratio()
        else:
            s_art = 0.8
        s = 0.65 * s_title + 0.35 * s_art
        if album and norm(album) == f["n_album"]:
            s += 0.05
        return min(s, 1.0)

    @staticmethod
    def _same_song(title, f):
        rb = norm(split_version(title)[0])
        return bool(rb) and (rb == f["n_base"] or (len(rb) >= 6 and SequenceMatcher(None, rb, f["n_base"]).ratio() >= 0.92))

    def _rank(self, idxs, artist, title, album, prefer):
        pref_order = {"flac": ["flac", "m4a", "opus", "mp3"], "mp3": ["mp3", "m4a", "opus", "flac"]}.get(prefer, [])
        def fmt_rank(f):
            return pref_order.index(f["ext"]) if f["ext"] in pref_order else len(pref_order)
        scored = []
        for i in idxs:
            f = self.files[i]
            sc = self._score(f, artist, title, album)
            scored.append((sc, f))
        # higher score first; among equal scores: chosen format, then the original studio album
        # (not live / best-of / remix sets), earliest year, shortest path
        # "mp3" usually means "my device can't play FLAC", so format wins there; for "flac", the
        # original album beats a FLAC live/remix version
        if prefer == "mp3":
            key = lambda x: (-round(x[0], 3), fmt_rank(x[1]), album_rank(x[1]), len(x[1]["rel"]))
        else:
            key = lambda x: (-round(x[0], 3), album_rank(x[1])[0], fmt_rank(x[1]), album_rank(x[1])[1], len(x[1]["rel"]))
        scored.sort(key=key)
        return scored

    def match(self, req, prefer="flac"):
        """req: {artist,title,album,path,raw}  ->  result dict"""
        artist = (req.get("artist") or "").strip()
        title = (req.get("title") or "").strip()
        album = (req.get("album") or "").strip()
        path = (req.get("path") or "").strip()
        raw = (req.get("raw") or "").strip()
        label = req.get("label") or raw or " - ".join(x for x in (artist, title) if x) or path

        res = {"row": req.get("row"), "label": label, "status": "missing",
               "file": None, "alternatives": [], "suggestions": [], "note": ""}

        def finish(scored, status, note=""):
            seen, alts = set(), []
            for sc, f in scored:
                if f["path"] in seen:
                    continue
                seen.add(f["path"])
                alts.append(dict(pub(f), score=round(sc, 3)))
            res["file"] = alts[0]
            res["alternatives"] = alts[1:25]
            res["status"] = status
            res["note"] = note
            return res

        # 1. a real path or a filename
        p = path or (raw if re.search(r"\.(%s)$" % "|".join(e[1:] for e in AUDIO_EXT), raw, re.I) else "")
        if p:
            p2 = os.path.expanduser(p.replace("\\", "/"))
            for cand in (p2, os.path.join(self.root, p2.lstrip("/"))):
                if isfile(cand) and os.path.splitext(cand)[1].lower() in AUDIO_EXT:
                    ap = os.path.abspath(cand)
                    f = next((x for x in self.files if x["path"] == ap), None) or file_info(ap, self.root)
                    return finish([(1.0, f)], "found")
            base = os.path.basename(p2).lower()
            hits = self.by_name.get(base) or self.by_name.get(os.path.splitext(base)[0])
            if hits:
                return finish([(1.0, self.files[i]) for i in hits], "found",
                              "same filename found in another folder" if path and "/" in p2 else "")
            # a playlist path from another computer: try to read artist/title out of the filename
            stem = os.path.splitext(os.path.basename(p2))[0]
            m = FNAME_RE.match(stem)
            if m:
                artist, album, title = artist or m["artist"], album or m["album"], title or m["title"]
            elif not title:
                raw = stem

        # 2. free text "Artist - Title" (or just words)
        if not title and raw:
            bits = [b.strip() for b in re.split(r"\s+[-–—]\s+|\t", raw) if b.strip()]
            if len(bits) >= 2:
                artist, title = bits[0], bits[-1]
            else:
                title = raw
                artist = ""

        # (words to search for) -> always scored against what was actually asked for
        attempts = []
        base = split_version(title)[0] or title
        for a, t, sa, st in ((artist, title, artist, title),
                             (artist, clean_title(title), artist, title),
                             (artist, base, artist, title),     # so other versions of the song are found too
                             (first_artist(artist), clean_title(title), artist, title),
                             (first_artist(artist), base, artist, title)):
            if (a, t, sa, st) not in attempts:
                attempts.append((a, t, sa, st))
        if not artist and raw:
            attempts.append(("", raw, "", raw))
        # also "Title - Artist" written backwards
        if artist and title:
            attempts.append((title, artist, title, artist))

        best = None
        for a, t, sa, st in attempts:
            idxs = self._candidates(tokens(a) + tokens(t))
            if not idxs:
                continue
            scored = self._rank(idxs, sa, st, album, prefer)
            if best is None or scored[0][0] > best[0][0]:
                best = scored
            if scored[0][0] >= 0.97:
                break

        if best and best[0][0] >= 0.6 and title:
            top = best[0][1]
            if self._same_song(title, top) and not same_version(split_version(title)[1], top["vtags"]) \
                    and norm(title) != top["n_title"]:
                return finish(best, "version", "you don't have this version - only the one shown here "
                                                "(or pick another from the list). Not copied unless you tick it.")
        if best and best[0][0] >= 0.94:
            return finish(best, "found")
        if best and best[0][0] >= 0.6:
            return finish(best, "guess", "not an exact match - check it's the right song")

        # 3. nothing contains all the words: look for near misses (typos, missing words)
        sugg = self._fuzzy(artist, title or raw, album, prefer)
        if sugg and sugg[0][0] >= 0.82 and best is None:
            return finish(sugg, "guess", "spelled differently in your library - check it's the right song")
        pool = (best or []) + sugg
        pool.sort(key=lambda x: -x[0])
        seen = set()
        na = norm(artist)
        rb = norm(split_version(title or raw)[0])
        def related(f):   # don't suggest a Rammstein song for a Queen song just because both say "mix"
            if not na:
                return True
            if na in f["n_artist"] or f["n_artist"] in na or na in f.get("n_dirartist", "") \
                    or SequenceMatcher(None, na, f["n_artist"]).ratio() >= 0.7:
                return True
            return bool(rb) and SequenceMatcher(None, rb, f.get("n_base", "")).ratio() >= 0.85
        for sc, f in pool:
            if f["path"] not in seen and sc >= 0.45 and related(f):
                seen.add(f["path"])
                res["suggestions"].append(dict(pub(f), score=round(sc, 3)))
            if len(res["suggestions"]) >= 5:
                break
        return res

    def _fuzzy(self, artist, title, album, prefer):
        q = tokens(artist) + tokens(title)
        if not q:
            return []
        hits = Counter()
        n = len(self.files) or 1
        for t in set(q):
            p = self.index.get(t)
            if p and len(p) < max(n * 0.2, 500):   # ignore words that are everywhere ("the", "mix")
                hits.update(p)
        # words that aren't in the library at all are probably typos: count close spellings instead
        from difflib import get_close_matches
        vocab = None
        for t in set(q):
            if len(t) >= 4 and t not in self.index:
                vocab = vocab or list(self.index.keys())
                for alt in get_close_matches(t, vocab, n=3, cutoff=0.75):
                    hits.update(self.index[alt])
        need = max(1, int(len(set(q)) * 0.5))
        idxs = [i for i, c in hits.most_common(4000) if c >= need]
        if not idxs:
            return []
        return self._rank(idxs, artist, title, album, prefer)[:10]


NOT_ORIGINAL = re.compile(r"\d{4}-\d\d-\d\d|\b(live|tour\w*|concert|festival|arena|stadium|unplugged|sessions?|best of|greatest|hits|collection|anthology|essential|"
                          r"singles|rarities|remix(es)?|mixes|b.sides|demos?|karaoke|instrumentals?|compilation|vol\.?\s*\d)\b|"
                          r"\b(cd|disc)\s*\d", re.I)

def album_rank(f):
    rel = f.get("rel", "")
    parent = os.path.dirname(rel)
    m = re.search(r"\((\d{4})\)", parent)
    year = int(m[1]) if m else 9999
    return (1 if NOT_ORIGINAL.search(parent) else 0, year)

def file_info(path, root):
    fn = os.path.basename(path)
    stem, ext = os.path.splitext(fn)
    return {"path": path, "rel": os.path.relpath(path, root) if path.startswith(root) else path,
            "name": fn, "ext": ext.lower().lstrip("."), "artist": "", "album": "", "title": stem,
            "size": getsize(path)}

def pub(f):
    return {k: f[k] for k in ("path", "rel", "name", "ext", "artist", "album", "title", "size")}

# ----------------------------------------------------------------------------- reading the list

HEAD = {
    "artist": {"artist", "artists", "artist name", "artist names", "artist name s", "artist s", "performer", "band", "singer", "by"},
    "title": {"title", "song", "songs", "track", "track name", "song name", "song title", "track title", "name", "tracks"},
    "album": {"album", "album name", "album title", "release"},
    "path": {"path", "file", "filename", "file name", "location", "filepath", "file path"},
}

def read_rows(filename, data):
    """-> list of request dicts"""
    ext = os.path.splitext(filename.lower())[1]
    if ext == ".xlsx":
        table = read_xlsx(data)
    elif ext in (".xls", ".numbers", ".ods"):
        raise ValueError(f"{ext} files can't be read - open it and 'Save as' CSV or .xlsx")
    else:
        text = decode(data)
        if ext in (".m3u", ".m3u8") or text.lstrip().startswith("#EXTM3U"):
            return read_m3u(text)
        first = text.lstrip().split("\n", 1)[0]
        header_like = any(norm(c) in names for c in re.split(r"[,;\t]", first) for names in HEAD.values())
        # a spreadsheet: .csv/.tsv file, rows pasted from Excel/Sheets (tabs), or text with a header row
        if ext in (".csv", ".tsv") or "\t" in first or (header_like and re.search(r"[,;]", first)):
            dialect = "excel-tab" if ext == ".tsv" or "\t" in first else "excel"
            if dialect == "excel":
                try:
                    dialect = csv.Sniffer().sniff(text[:4096], delimiters=",;\t")
                except csv.Error:
                    pass
            table = list(csv.reader(io.StringIO(text), dialect))
        else:
            return [{"row": n, "raw": line.strip()} for n, line in enumerate(text.splitlines(), 1)
                    if line.strip() and not line.strip().startswith("#")]
    return rows_from_table(table)

def decode(data):
    for enc in ("utf-8-sig", "utf-16", "cp1252", "latin-1"):
        try:
            t = data.decode(enc)
            if enc == "utf-16" and not data[:2] in (b"\xff\xfe", b"\xfe\xff"):
                continue
            return t
        except (UnicodeDecodeError, UnicodeError):
            continue
    return data.decode("utf-8", "replace")

def rows_from_table(table):
    table = [[(c or "").strip() for c in r] for r in table]
    table = [r for r in table if any(r)]
    if not table:
        return []
    # find a header row in the first 5 rows
    cols, start = None, 0
    for hi, r in enumerate(table[:5]):
        found = {}
        for ci, c in enumerate(r):
            k = norm(c)
            for field, names in HEAD.items():
                if k in names and field not in found:
                    found[field] = ci
        if "title" in found or "path" in found:
            cols, start = found, hi + 1
            break
    out = []
    for n, r in enumerate(table[start:], start + 1):
        get = lambda f: r[cols[f]] if cols and f in cols and cols[f] < len(r) else ""
        if cols:
            req = {"row": n, "artist": get("artist"), "title": get("title"), "album": get("album"), "path": get("path")}
            if not (req["title"] or req["path"]):
                continue
        else:
            cells = [c for c in r if c]
            if len(cells) == 1:
                req = {"row": n, "raw": cells[0]}
            else:  # no header, several columns: guess Artist, Title[, Album]
                req = {"row": n, "artist": cells[0], "title": cells[1], "album": cells[2] if len(cells) > 2 else ""}
        out.append(req)
    return out

def read_m3u(text):
    out, extinf = [], None
    for n, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        if line.upper().startswith("#EXTINF"):
            extinf = line.split(",", 1)[1].strip() if "," in line else None
            continue
        if line.startswith("#"):
            continue
        req = {"row": n, "path": line, "label": extinf or os.path.basename(line)}
        if extinf and " - " in extinf:
            a, t = extinf.split(" - ", 1)
            req["artist"], req["title"] = a.strip(), t.strip()
        out.append(req)
        extinf = None
    return out

def read_xlsx(data):
    ns = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    z = zipfile.ZipFile(io.BytesIO(data))
    shared = []
    if "xl/sharedStrings.xml" in z.namelist():
        for si in ET.fromstring(z.read("xl/sharedStrings.xml")).findall("m:si", ns):
            shared.append("".join(t.text or "" for t in si.iter("{%s}t" % ns["m"])))
    sheets = sorted(n for n in z.namelist() if re.match(r"xl/worksheets/sheet\d+\.xml$", n))
    if not sheets:
        return []
    root = ET.fromstring(z.read(sheets[0]))
    table = []
    for row in root.iter("{%s}row" % ns["m"]):
        cells = {}
        for c in row.findall("m:c", ns):
            ref = c.get("r", "")
            col = 0
            for ch in re.match(r"[A-Z]*", ref).group():
                col = col * 26 + ord(ch) - 64
            t = c.get("t")
            v = c.find("m:v", ns)
            if t == "s" and v is not None:
                val = shared[int(v.text)]
            elif t == "inlineStr":
                val = "".join(x.text or "" for x in c.iter("{%s}t" % ns["m"]))
            else:
                val = v.text if v is not None else ""
            cells[(col or len(cells) + 1) - 1] = val or ""
        if cells:
            table.append([cells.get(i, "") for i in range(max(cells) + 1)])
    return table

# ----------------------------------------------------------------------------- copying

BAD_CHARS = re.compile(r'[<>:"|?*\x00-\x1f]')

def plan_copies(items, dest, folders=False, number=False):
    """items: [{"src":..., "rel":...}] in list order -> [(src, dst)]; never two sources to one dst"""
    plan, used, seen_src = [], set(), set()
    # "n" = the song's position in the whole list, so numbers stay the same when a later run
    # only copies the songs that were missing before
    width = max(3, len(str(max([it.get("n") or 0 for it in items] + [len(items)]))))
    for k, it in enumerate(items, 1):
        n = it.get("n") or k
        src = it["src"]
        if src in seen_src:
            continue
        seen_src.add(src)
        name = os.path.basename(src)
        if number:
            name = f"{n:0{width}d} - {name}"
        name = BAD_CHARS.sub("_", name)   # so it also works on USB sticks / Windows
        if folders and it.get("rel") and not os.path.isabs(it["rel"]):
            sub = os.path.dirname(it["rel"])
            sub = os.sep.join(BAD_CHARS.sub("_", p) for p in sub.split(os.sep) if p)
            dst = os.path.join(dest, sub, name)
        else:
            dst = os.path.join(dest, name)
        base, ext = os.path.splitext(dst)
        k = 2
        while dst in used:
            dst = f"{base} ({k}){ext}"
            k += 1
        used.add(dst)
        plan.append((src, dst))
    return plan

def copy_one(src, dst):
    """-> ("copied"|"skipped"|"error", final_dst, message). Never overwrites."""
    try:
        os.makedirs(longp(os.path.dirname(dst)), exist_ok=True)
        if os.path.exists(longp(dst)):
            if getsize(dst) == getsize(src):
                return "skipped", dst, "already there"
            base, ext = os.path.splitext(dst)
            k = 2
            while os.path.exists(longp(f"{base} ({k}){ext}")):
                k += 1
            dst = f"{base} ({k}){ext}"
        # open with 'x' so we can never clobber a file that appeared meanwhile
        with open(longp(src), "rb") as fi, open(longp(dst), "xb") as fo:
            shutil.copyfileobj(fi, fo, 4 * 1024 * 1024)
        try:
            shutil.copystat(longp(src), longp(dst))
        except OSError:
            pass  # e.g. FAT32 USB sticks; the music itself is copied fine
        return "copied", dst, ""
    except Exception as e:  # noqa: BLE001 - report every failure, keep going
        return "error", dst, str(e)

# ----------------------------------------------------------------------------- USB sticks for CDJs / XDJs

# What every Pioneer/AlphaTheta deck with a USB port plays (CDJ-900/2000, XDJ-1000 MK1 and newer).
# FLAC/ALAC only arrived with the Nexus 2 generation; Opus/Ogg/WMA never. Hi-res (>48 kHz) isn't safe.
DECK_SAFE = {"mp3": {"mp3"}, "aac": {"m4a", "mp4", "aac"}, "pcm_s16le": {"wav"}, "pcm_s24le": {"wav"},
             "pcm_s16be": {"aif", "aiff"}, "pcm_s24be": {"aif", "aiff"}}
USB_FORMATS = {"aiff": ".aiff", "mp3": ".mp3"}

def ffmpeg_path():
    """(ffmpeg, ffprobe) - on PATH, or where `winget install Gyan.FFmpeg` puts them"""
    def find(name):
        p = shutil.which(name)
        if p or not WINDOWS:
            return p
        la = os.environ.get("LOCALAPPDATA", "")
        link = os.path.join(la, "Microsoft", "WinGet", "Links", name + ".exe")
        if os.path.isfile(link):
            return link
        import glob
        hits = glob.glob(os.path.join(la, "Microsoft", "WinGet", "Packages", "Gyan.FFmpeg*", "*", "bin", name + ".exe"))
        return hits[0] if hits else None
    return find("ffmpeg"), find("ffprobe")

FFMPEG_INSTALL = {"running": False, "log": ""}

def install_ffmpeg():
    """Windows: install ffmpeg with winget (free, no admin needed)"""
    import subprocess
    FFMPEG_INSTALL.update(running=True, log="")
    try:
        r = subprocess.run(["winget", "install", "-e", "--id", "Gyan.FFmpeg", "--scope", "user",
                            "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"],
                           capture_output=True, timeout=900, **NO_WINDOW)
        FFMPEG_INSTALL["log"] = (r.stdout + r.stderr).decode(errors="replace")[-600:]
    except Exception as e:  # noqa: BLE001
        FFMPEG_INSTALL["log"] = f"couldn't run winget: {e}"
    FFMPEG_INSTALL["running"] = False

def probe(src):
    """-> {"codec", "rate", "bits"} of the first audio stream (None if ffprobe is missing / fails)"""
    import subprocess
    ffprobe = ffmpeg_path()[1]
    if not ffprobe:
        return None
    try:
        out = subprocess.run([ffprobe, "-v", "error", "-select_streams", "a:0", "-show_entries",
                              "stream=codec_name,sample_rate,bits_per_raw_sample,bits_per_sample",
                              "-of", "json", "file:" + longp(src)],
                             capture_output=True, timeout=60, **NO_WINDOW)
        s = json.loads(out.stdout or b"{}").get("streams", [{}])[0]
    except Exception:  # noqa: BLE001
        return None
    bits = int(s.get("bits_per_raw_sample") or s.get("bits_per_sample") or 0)
    return {"codec": s.get("codec_name", ""), "rate": int(s.get("sample_rate") or 0), "bits": bits}

def deck_playable(src, info):
    ext = os.path.splitext(src)[1].lower().lstrip(".")
    return (info is not None and ext in DECK_SAFE.get(info["codec"], ())
            and info["rate"] in (44100, 48000) and info["bits"] in (0, 16, 24))

def convert_one(src, dst, fmt="aiff"):
    """deck-ready copy of src at dst (extension decided here). -> (status, final_dst, message).
    Like copy_one: never overwrites, never touches src."""
    import subprocess
    info = probe(src)
    if info is not None and deck_playable(src, info):
        return copy_one(src, dst)
    ffmpeg = ffmpeg_path()[0]
    if not ffmpeg:
        return "error", dst, "this file needs converting for the decks, but ffmpeg isn't installed"
    dst = os.path.splitext(dst)[0] + USB_FORMATS[fmt]
    try:
        os.makedirs(longp(os.path.dirname(dst)), exist_ok=True)
        base, ext = os.path.splitext(dst)
        k = 2
        while os.path.exists(longp(dst)):
            dst = f"{base} ({k}){ext}"
            k += 1
        rate = (info or {}).get("rate") or 44100
        rate = rate if rate in (44100, 48000) else (48000 if rate % 48000 == 0 else 44100)
        if fmt == "aiff":
            codec = ["-c:a", "pcm_s24be" if (info or {}).get("bits", 16) > 16 else "pcm_s16be", "-write_id3v2", "1", "-f", "aiff"]
        else:
            codec = ["-c:a", "libmp3lame", "-b:a", "320k", "-id3v2_version", "3", "-f", "mp3"]
        part = dst + ".part"
        for art in (["-map", "0:v:0?", "-c:v", "copy", "-disposition:v", "attached_pic"], []):  # retry without cover art
            cmd = [ffmpeg, "-v", "error", "-nostdin", "-y", "-i", "file:" + longp(src), "-map", "0:a:0", *art,
                   "-ar", str(rate), "-ac", "2", "-map_metadata", "0", *codec, "file:" + longp(part)]
            r = subprocess.run(cmd, capture_output=True, timeout=900, **NO_WINDOW)
            if r.returncode == 0:
                break
        if r.returncode != 0:
            try:
                os.remove(longp(part))
            except OSError:
                pass
            return "error", dst, "conversion failed: " + r.stderr.decode(errors="replace").strip()[-300:]
        if os.path.exists(longp(dst)):   # appeared while converting: still never overwrite
            os.remove(longp(part))
            return "error", dst, "a file with that name appeared while converting"
        os.replace(longp(part), longp(dst))
        return "converted", dst, f"{(info or {}).get('codec', '?')} → {fmt.upper()}"
    except Exception as e:  # noqa: BLE001
        return "error", dst, str(e)

NO_WINDOW = {"creationflags": 0x08000000} if WINDOWS else {}   # no console windows popping up per file

# ----------------------------------------------------------------------------- remembering what was copied

class Ledger:
    """What songcopy copied to each destination, so a re-run of the same list (after you've added the
    missing songs to your library) only offers what's still to do. Kept in the user's cache folder -
    nothing extra is written into the destination. A song counts as copied only while its copy still exists."""
    lock = threading.Lock()

    @staticmethod
    def _file():
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~/.cache")
        return os.path.join(base, "songcopy", "copied.json")

    @staticmethod
    def _key(dest):
        return os.path.normcase(os.path.abspath(os.path.expanduser(dest)))

    @classmethod
    def _load(cls):
        try:
            with open(cls._file(), encoding="utf-8") as fh:
                return json.load(fh)
        except (OSError, ValueError):
            return {}

    @classmethod
    def record(cls, dest, pairs):
        """pairs: [(src, final_dst)]"""
        if not pairs:
            return
        with cls.lock:
            d = cls._load()
            m = d.setdefault(cls._key(dest), {})
            for src, dst in pairs:
                m[src] = dst
            p = cls._file()
            try:
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p + ".tmp", "w", encoding="utf-8") as fh:
                    json.dump(d, fh)
                os.replace(p + ".tmp", p)
            except OSError:
                pass

    @classmethod
    def copied(cls, dest, srcs):
        """-> {src: dst} for the srcs already copied to dest (and still there)"""
        if not dest or not dest.strip():
            return {}
        dest = os.path.abspath(os.path.expanduser(dest))
        m = cls._load().get(cls._key(dest), {})
        out = {}
        for s in srcs:
            d = m.get(s)
            if d and os.path.exists(longp(d)):
                out[s] = d
                continue
            # copied before songcopy kept a record (or by hand): same name + size right in dest
            d = os.path.join(dest, BAD_CHARS.sub("_", os.path.basename(s)))
            try:
                if os.path.exists(longp(d)) and getsize(d) == getsize(s):
                    out[s] = d
            except OSError:
                pass
        return out

def transfer(plan, dest_key, usb_format=None, on_progress=None):
    """copy (or for a USB, copy/convert) every (src, dst) in plan. Returns a summary dict."""
    res = {"copied": 0, "converted": 0, "skipped": 0, "errors": [], "done": [], "pairs": []}
    for k, (s, d) in enumerate(plan, 1):
        if on_progress:
            on_progress(k, s)
        st, final, msg = convert_one(s, d, usb_format) if usb_format else copy_one(s, d)
        if st == "error":
            res["errors"].append({"src": s, "msg": msg})
            continue
        res[st] += 1
        res["done"].append(final)
        res["pairs"].append((s, final))
        if len(res["pairs"]) % 20 == 0:   # save as we go, so an interrupted copy still counts
            Ledger.record(dest_key, res["pairs"][-20:])
    Ledger.record(dest_key, res["pairs"][len(res["pairs"]) // 20 * 20:])
    return res

def usb_folder(dest, name):
    name = BAD_CHARS.sub("_", (name or "").strip()).strip(". ") or "songcopy"
    return os.path.join(dest, name[:80])

def estimate_size(plan, usb_format=None):
    """bytes needed; FLAC -> AIFF is roughly 1.7x, lossy -> AIFF a lot more"""
    total = 0
    for s, _ in plan:
        size = getsize(s)
        ext = os.path.splitext(s)[1].lower()
        if usb_format == "aiff" and ext in (".flac", ".alac", ".ape", ".wv"):
            size *= 1.8
        elif usb_format == "aiff" and ext in (".opus", ".ogg", ".wma"):
            size *= 10
        total += size
    return int(total)

def write_playlist(dest, done):
    path = os.path.join(dest, "playlist.m3u8")
    base, k = path, 2
    while os.path.exists(path):
        path = base.replace(".m3u8", f" ({k}).m3u8")
        k += 1
    with open(path, "x", encoding="utf-8") as fh:
        fh.write("#EXTM3U\n")
        for d in done:
            fh.write(os.path.relpath(d, dest) + "\n")
    return path

def write_missing_csv(path, results):
    with open(path, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["Row", "Song", "Problem", "Closest thing in your library"])
        for r in results:
            if r["status"] == "missing":
                s = r["suggestions"][0]["rel"] if r["suggestions"] else ""
                w.writerow([r["row"], r["label"], "not found in library", s])
            elif r["status"] == "version":
                w.writerow([r["row"], r["label"], "only a different version in library", r["file"]["rel"]])
            elif r["status"] == "error":
                w.writerow([r["row"], r["label"], "copy failed: " + r.get("note", ""), ""])

# ----------------------------------------------------------------------------- terminal mode

if WINDOWS:
    os.system("")   # turn on colours in the Windows console
try:
    sys.stdout.reconfigure(errors="replace")
except Exception:  # noqa: BLE001
    pass

class C:
    on = sys.stdout.isatty()
    R = "\033[31m" if on else ""; G = "\033[32m" if on else ""; Y = "\033[33m" if on else ""
    M = "\033[35m" if on else ""; B = "\033[1m" if on else ""; D = "\033[2m" if on else ""; X = "\033[0m" if on else ""

def cli_run(args, do_copy):
    if not os.path.isfile(args.list):
        sys.exit(f"{C.R}Can't find the list file: {args.list}{C.X}")
    lib = Library(args.music)
    print(f"{C.D}Reading your library {lib.root} ...{C.X}", end="", flush=True)
    t0 = time.time()
    n = lib.scan(quiet=False)
    print(f"\r{C.D}Read your library {lib.root}: {n:,} songs ({time.time()-t0:.1f}s){C.X}        ")
    with open(args.list, "rb") as fh:
        try:
            reqs = read_rows(args.list, fh.read())
        except ValueError as e:
            sys.exit(f"{C.R}{e}{C.X}")
    if not reqs:
        sys.exit(f"{C.R}No songs found in {args.list}{C.X}")
    print(f"{C.D}Looking up {len(reqs)} songs from {os.path.basename(args.list)} ...{C.X}\n")
    results = [lib.match(r, args.prefer) for r in reqs]
    for i, r in enumerate(results, 1):
        r["n"] = i

    found = [r for r in results if r["status"] == "found"]
    guess = [r for r in results if r["status"] == "guess"]
    missing = [r for r in results if r["status"] == "missing"]
    other = [r for r in results if r["status"] == "version"]
    any_version = getattr(args, "any_version", False)

    if not do_copy and found:
        print(f"{C.G}{C.B}Found ({len(found)}):{C.X}")
        for r in found:
            extra = f"  (+{len(r['alternatives'])} other versions)" if r["alternatives"] else ""
            print(f"  {C.G}row {r['row']:>4}{C.X}  {r['label']}\n            {C.D}→ {r['file']['rel']}{extra}{C.X}")
        print()

    if guess:
        print(f"{C.Y}{C.B}Best guesses ({len(guess)}) - not exact, please check:{C.X}")
        for r in guess:
            print(f"  {C.Y}row {r['row']:>4}{C.X}  {r['label']}\n            {C.D}→ {r['file']['rel']}{C.X}")
        print(f"  {C.D}({'skipped because of --strict' if args.strict else 'these WILL be copied; use --strict to skip them'}){C.X}\n")

    if other:
        print(f"{C.M}{C.B}Only a DIFFERENT VERSION in your library ({len(other)}):{C.X}")
        for r in other:
            print(f"  {C.M}row {r['row']:>4}  {r['label']}{C.X}\n            {C.D}you have: {r['file']['rel']}{C.X}")
        print(f"  {C.D}({'these WILL be copied (--any-version)' if any_version else 'NOT copied; add --any-version to copy what you have instead'}){C.X}\n")

    if missing:
        print(f"{C.R}{C.B}NOT FOUND in {lib.root} ({len(missing)}):{C.X}")
        for r in missing:
            print(f"  {C.R}row {r['row']:>4}  {r['label']}{C.X}")
            if r["suggestions"]:
                print(f"            {C.D}closest: {r['suggestions'][0]['rel']}{C.X}")
        print()

    to_copy = found + ([] if args.strict else guess) + (other if any_version else [])
    to_copy.sort(key=lambda r: r["row"] or 0)
    items = [{"src": r["file"]["path"], "rel": r["file"]["rel"], "n": r["n"]} for r in to_copy]

    print(f"{C.B}Found {len(found)} exact, {len(guess)} best-guess, "
          + (f"{C.M}{len(other)} other-version-only{C.X}{C.B}, " if other else "")
          + f"{C.R if missing else ''}{len(missing)} missing{C.X}{C.B} (of {len(results)}){C.X}")

    missing_csv = None
    if missing or other:
        missing_csv = os.path.splitext(os.path.abspath(args.list))[0] + "-missing.csv"
        write_missing_csv(missing_csv, results)
        print(f"Missing{' / other-version' if other else ''} list saved to: {C.B}{missing_csv}{C.X}")

    if not do_copy:
        return 1 if (missing or other) else 0

    dest = os.path.abspath(os.path.expanduser(args.dest))
    before = {} if args.recopy else Ledger.copied(dest, [it["src"] for it in items])
    if before:
        items = [it for it in items if it["src"] not in before]
        print(f"{C.G}{len(before)} already copied to {dest} on an earlier run - skipped{C.X} {C.D}(--recopy to copy them again){C.X}")
        if not items:
            print(f"{C.G}{C.B}Nothing new to copy.{C.X}")
            return 1 if (missing or other) else 0

    usb_format = args.usb_format if args.usb else None
    if usb_format:
        if not all(ffmpeg_path()):
            print(f"{C.Y}ffmpeg isn't installed: songs the decks can't play (FLAC etc.) will fail. Install ffmpeg first.{C.X}")
        target = usb_folder(dest, args.name or os.path.splitext(os.path.basename(args.list))[0])
        plan = plan_copies(items, target, False, True)
    else:
        target = dest
        plan = plan_copies(items, dest, args.folders, args.number)
    total = estimate_size(plan, usb_format)
    verb = ("Would copy" if args.dry_run else "Copying") + (f" (for decks: anything they can't play → {usb_format.upper()})" if usb_format else "")
    print(f"\n{verb} {len(plan)} files (about {human(total)}) to {C.B}{target}{C.X}")
    free = shutil.disk_usage(nearest_existing(dest)).free
    if total > free:
        sys.exit(f"{C.R}Not enough space: need about {human(total)}, only {human(free)} free on that drive.{C.X}")
    if args.dry_run:
        for s, d in plan[:20]:
            print(f"  {C.D}{os.path.relpath(d, dest)}{C.X}")
        if len(plan) > 20:
            print(f"  {C.D}... and {len(plan)-20} more{C.X}")
        print(f"{C.D}(dry run: nothing was copied){C.X}")
        return 1 if missing else 0

    def progress(k, s):
        if C.on:
            print(f"\r  {k}/{len(plan)}  {os.path.basename(s)[:60]:<60}", end="", flush=True)
    r = transfer(plan, dest, usb_format, progress)
    errors = r["errors"]
    if C.on:
        print("\r" + " " * 80 + "\r", end="")
    if args.playlist and r["done"]:
        print(f"Playlist: {write_playlist(target, r['done'])}")
    print(f"{C.G}{C.B}Done: {r['copied'] + r['converted']} copied{C.X}"
          + (f" ({r['converted']} converted for the decks)" if r["converted"] else "")
          + (f", {r['skipped']} already there (skipped)" if r["skipped"] else ""))
    if errors:
        print(f"{C.R}{C.B}{len(errors)} could not be copied:{C.X}")
        for e in errors:
            print(f"  {C.R}{e['src']}\n    {e['msg']}{C.X}")
    if missing:
        print(f"{C.R}{len(missing)} song{'s were' if len(missing) > 1 else ' was'} not in your library - see {missing_csv}{C.X}")
    return 1 if (missing or errors) else 0

def human(n):
    for u in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or u == "TB":
            return f"{n:.1f} {u}" if u != "B" else f"{n} B"
        n /= 1024

def nearest_existing(p):
    while p and not os.path.exists(p):
        p = os.path.dirname(p)
    return p or "/"

# ----------------------------------------------------------------------------- web mode

JOBS = {}

class Handler(BaseHTTPRequestHandler):
    lib: Library = None
    default_dest = None

    def log_message(self, *a):
        pass

    def send(self, code, body, ctype="application/json"):
        if not isinstance(body, (bytes, str)):
            body = json.dumps(body)
        if isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype + ("; charset=utf-8" if "json" in ctype or "html" in ctype else ""))
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        lib = self.lib
        if u.path == "/":
            return self.send(200, PAGE, "text/html")
        if u.path == "/api/info":
            return self.send(200, {"root": lib.root, "count": len(lib.files), "ready": lib.ready.is_set(),
                                   "scanning": lib.scanning, "progress": lib.progress,
                                   "scanned": when(lib.scanned_at), "ffmpeg": all(ffmpeg_path()), "windows": WINDOWS, "version": VERSION,
                                   "ffmpeg_installing": FFMPEG_INSTALL["running"], "ffmpeg_log": FFMPEG_INSTALL["log"],
                                   "home": os.path.expanduser("~"), "dest": self.default_dest or "",
                                   "example": os.path.join(os.path.expanduser("~"), "Desktop", "My songs")})
        if u.path == "/api/ls":
            raw = q.get("path", ["~"])[0]
            if WINDOWS and raw == "":   # "This PC": list the drives
                drives = [f"{d}:\\" for d in "CDEFGHIJKLMNOPQRSTUVWXYZ" if os.path.exists(f"{d}:\\")]
                return self.send(200, {"path": "", "parent": None, "free": "",
                                       "dirs": [{"name": d, "path": d} for d in drives]})
            p = os.path.abspath(os.path.expanduser(raw or "~"))
            ex = nearest_existing(p)
            try:
                names = sorted((d for d in os.listdir(ex) if not d.startswith((".", "$"))
                                and os.path.isdir(os.path.join(ex, d))), key=str.lower)
            except OSError as e:
                return self.send(200, {"path": ex, "dirs": [], "error": str(e)})
            parent = os.path.dirname(ex)
            if parent == ex:   # at the top of a drive
                parent = "" if WINDOWS else None
            return self.send(200, {"path": ex, "exists": ex == p, "parent": parent,
                                   "dirs": [{"name": d, "path": os.path.join(ex, d)} for d in names[:500]],
                                   "free": human(shutil.disk_usage(ex).free)})
        if u.path.startswith("/api/job/"):
            j = JOBS.get(u.path.rsplit("/", 1)[1])
            return self.send(200 if j else 404, j or {"error": "no such job"})
        if u.path == "/api/play":   # preview a song in the browser
            p = q.get("path", [""])[0]
            if not p.startswith(lib.root + os.sep) or not isfile(p) or os.path.splitext(p)[1].lower() not in AUDIO_EXT:
                return self.send(404, {"error": "not found"})
            return self.stream(p)
        self.send(404, {"error": "not found"})

    def stream(self, p):
        size = getsize(p)
        ctype = {".mp3": "audio/mpeg", ".flac": "audio/flac", ".m4a": "audio/mp4", ".opus": "audio/ogg",
                 ".ogg": "audio/ogg", ".wav": "audio/wav"}.get(os.path.splitext(p)[1].lower(), "application/octet-stream")
        start, end = 0, size - 1
        rng = self.headers.get("Range")
        if rng and (m := re.match(r"bytes=(\d*)-(\d*)", rng)):
            if m[1]:
                start = int(m[1]); end = int(m[2]) if m[2] else end
            elif m[2]:
                start = size - int(m[2])
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        else:
            self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        try:
            with open(longp(p), "rb") as fh:
                fh.seek(start)
                left = end - start + 1
                while left > 0:
                    chunk = fh.read(min(1 << 16, left))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    left -= len(chunk)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        u = urlparse(self.path)
        lib = self.lib
        try:
            b = self.body()
        except Exception:
            return self.send(400, {"error": "bad request"})
        if u.path == "/api/rescan":
            if not lib.scanning:
                threading.Thread(target=lib.scan, daemon=True).start()
            return self.send(200, {"started": True})
        if u.path == "/api/match":
            try:
                if b.get("text") is not None:
                    reqs = read_rows(b.get("filename") or "pasted.txt", b["text"].encode())
                else:
                    reqs = read_rows(b.get("filename", "list.csv"), base64.b64decode(b.get("data", "")))
            except Exception as e:  # noqa: BLE001
                return self.send(200, {"error": f"Couldn't read that file: {e}"})
            if not reqs:
                return self.send(200, {"error": "No songs found in that list."})
            prefer = b.get("prefer", "flac")
            lib.ready.wait()   # first-ever run: wait until the library has been read
            with lib.lock:
                results = [lib.match(r, prefer) for r in reqs]
            return self.send(200, {"results": results})
        if u.path == "/api/search":   # manual search box for a missing song
            q = b.get("q", "")
            lib.ready.wait()
            with lib.lock:
                out = lib.search(q, b.get("prefer", "flac"))
            return self.send(200, {"files": out})
        if u.path == "/api/copy":
            dest = os.path.abspath(os.path.expanduser(b.get("dest", "").strip()))
            if not b.get("dest", "").strip():
                return self.send(200, {"error": "Choose a destination folder first."})
            if dest == lib.root or dest.startswith(lib.root + os.sep):
                pass  # allowed (copy into a subfolder of Music), still never overwrites
            items = [{"src": it["src"], "rel": it.get("rel", ""), "n": it.get("n")} for it in b.get("items", [])
                     if isfile(it.get("src", ""))]
            if not items:
                return self.send(200, {"error": "Nothing selected to copy."})
            usb_format = b.get("usb_format") if b.get("mode") == "usb" else None
            if usb_format and usb_format not in USB_FORMATS:
                return self.send(200, {"error": "Unknown USB format."})
            if usb_format:   # USB for decks: one playlist folder, numbered in list order
                target = usb_folder(dest, b.get("usb_name"))
                plan = plan_copies(items, target, False, True)
            else:
                target = dest
                plan = plan_copies(items, dest, b.get("folders"), b.get("number"))
            total = estimate_size(plan, usb_format)
            free = shutil.disk_usage(nearest_existing(dest)).free
            if total > free:
                return self.send(200, {"error": f"Not enough space: need about {human(total)}, only {human(free)} free there."})
            jid = uuid.uuid4().hex[:10]
            JOBS[jid] = {"total": len(plan), "n": 0, "copied": 0, "converted": 0, "skipped": 0, "errors": [],
                         "current": "", "finished": False, "dest": target, "playlist": None, "done_srcs": []}
            threading.Thread(target=run_job, args=(jid, plan, dest, target, b.get("playlist"), usb_format),
                             daemon=True).start()
            return self.send(200, {"job": jid, "count": len(plan), "size": human(total)})
        if u.path == "/api/install_ffmpeg" and WINDOWS:
            if not FFMPEG_INSTALL["running"] and not all(ffmpeg_path()):
                threading.Thread(target=install_ffmpeg, daemon=True).start()
            return self.send(200, {"started": True})
        if u.path == "/api/copied":   # which of these songs are already in that destination?
            return self.send(200, {"copied": list(Ledger.copied(b.get("dest", ""), b.get("srcs", [])))})
        self.send(404, {"error": "not found"})

def run_job(jid, plan, dest, target, playlist, usb_format):
    j = JOBS[jid]
    def progress(k, s):
        j["n"], j["current"] = k - 1, os.path.basename(s)
    try:
        r = transfer(plan, dest, usb_format, progress)
        j.update(copied=r["copied"], converted=r["converted"], skipped=r["skipped"], errors=r["errors"],
                 done_srcs=[s for s, _ in r["pairs"]])
        if playlist and r["done"]:
            try:
                j["playlist"] = write_playlist(target, r["done"])
            except OSError as e:
                j["errors"].append({"src": "playlist.m3u8", "msg": str(e)})
    except Exception as e:  # noqa: BLE001
        j["errors"].append({"src": "", "msg": f"stopped: {e}"})
    j["n"], j["current"], j["finished"] = len(plan), "", True

def when(dt):
    if not dt:
        return None
    return dt.strftime("%H:%M") if dt.date() == datetime.now().date() else dt.strftime("%d %b %H:%M")

def open_browser(url):
    """open the page in the default browser - but never a text browser inside this terminal"""
    if sys.platform.startswith("linux") and not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        return False
    import webbrowser
    try:
        return webbrowser.open(url, new=2)
    except Exception:  # noqa: BLE001
        return False

def already_running(port):
    import urllib.request
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/info", timeout=1) as r:
            return "root" in json.load(r)
    except Exception:  # noqa: BLE001
        return False

def serve(args):
    url = f"http://localhost:{args.port}"
    if not os.path.isdir(longp(os.path.abspath(os.path.expanduser(args.music)))):
        sys.exit(f"{C.R}Can't open the music folder: {args.music}{C.X}")
    try:
        srv = ThreadingHTTPServer((args.host, args.port), Handler)
    except OSError as e:
        if already_running(args.port):
            print(f"Song Copy is already running - opening {url}")
            if args.no_browser or not open_browser(url):
                print(f"Open {url} in your browser.")
            return
        sys.exit(f"Can't start on port {args.port}: {e}\nTry:  songcopy serve --port 8766")

    lib = Library(args.music)
    Handler.lib = lib
    Handler.default_dest = args.dest
    if lib.load_cache():
        print(f"{C.D}Using saved song list ({len(lib.files):,} songs); checking for new songs in the background.{C.X}")
    else:
        print(f"{C.D}Reading your library {lib.root} for the first time ...{C.X}")
    threading.Thread(target=lib.scan, daemon=True).start()

    print(f"\n  Song Copy is running at  {C.B}{url}{C.X}")
    if args.host == "0.0.0.0":
        print("  (also reachable from other devices on your network at this computer's IP)")
    print("  Leave this window open while you use it. Press Ctrl+C (or close it) to stop.\n")
    if not args.no_browser and not open_browser(url):
        print(f"  Open {url} in your browser.\n")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")

# ----------------------------------------------------------------------------- the web page

PAGE = r"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Song Copy</title>
<style>
:root{--bg:#f6f5f2;--card:#fff;--ink:#1d1d1f;--mute:#6b6b70;--line:#e3e1dc;--acc:#2f6fdf;--acc-ink:#fff;
--alt:#7b4bc4;--alt-bg:#f1eafb;--ok:#1f8a4c;--ok-bg:#e6f4ec;--warn:#a26300;--warn-bg:#fdf1dc;--bad:#c0392b;--bad-bg:#fbe7e4;--shadow:0 1px 2px rgba(0,0,0,.05),0 4px 16px rgba(0,0,0,.04)}
@media (prefers-color-scheme:dark){:root{--bg:#141416;--card:#1e1e21;--ink:#ececee;--mute:#9a9aa2;--line:#303036;--acc:#5b8ff0;
--alt:#b996f2;--alt-bg:#2a2140;--ok:#4cc27e;--ok-bg:#16301f;--warn:#e6a845;--warn-bg:#33270f;--bad:#f07060;--bad-bg:#3a1a17;--shadow:none}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.45 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
main{max-width:1060px;margin:0 auto;padding:28px 18px 80px}
h1{font-size:26px;margin:0 0 2px;letter-spacing:-.01em}
.sub{color:var(--mute);margin:0 0 22px}
.card{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:18px 20px;margin-bottom:16px;box-shadow:var(--shadow)}
.step{display:flex;gap:10px;align-items:baseline;margin:0 0 12px}
.step b{display:inline-grid;place-items:center;width:24px;height:24px;border-radius:50%;background:var(--acc);color:var(--acc-ink);font-size:13px;flex:none}
.step h2{font-size:17px;margin:0}
.drop{border:2px dashed var(--line);border-radius:12px;padding:26px;text-align:center;color:var(--mute);cursor:pointer;transition:.15s}
.drop.over,.drop:hover{border-color:var(--acc);color:var(--ink)}
.drop strong{color:var(--ink)}
textarea{width:100%;min-height:110px;margin-top:10px;border:1px solid var(--line);border-radius:10px;padding:10px;font:13px/1.4 ui-monospace,Menlo,monospace;background:var(--bg);color:var(--ink);resize:vertical}
input[type=text]{flex:1;min-width:0;border:1px solid var(--line);border-radius:9px;padding:9px 11px;font:inherit;background:var(--bg);color:var(--ink)}
button{font:inherit;border:1px solid var(--line);background:var(--card);color:var(--ink);border-radius:9px;padding:8px 14px;cursor:pointer}
button:hover{border-color:var(--acc)}
button.primary{background:var(--acc);border-color:var(--acc);color:var(--acc-ink);font-weight:600;padding:10px 18px}
button:disabled{opacity:.45;cursor:default}
.row{display:flex;gap:8px;align-items:center;flex-wrap:wrap}
.opts{display:flex;gap:18px;flex-wrap:wrap;margin-top:12px;color:var(--mute);font-size:14px}
.opts label{display:flex;gap:6px;align-items:center;cursor:pointer}
select{font:inherit;border:1px solid var(--line);border-radius:8px;padding:4px 6px;background:var(--bg);color:var(--ink);max-width:100%}
.browser{margin-top:8px;border:1px solid var(--line);border-radius:10px;max-height:220px;overflow:auto;display:none}
.browser div{padding:6px 12px;cursor:pointer;border-bottom:1px solid var(--line);font-size:14px}
.browser div:hover{background:var(--bg)}
.small{font-size:13px;color:var(--mute)}
.summary{display:flex;gap:10px;flex-wrap:wrap;margin-bottom:12px}
.pill{border-radius:999px;padding:6px 14px;font-weight:600;font-size:14px;cursor:pointer;border:2px solid transparent}
.pill.alt{background:var(--alt-bg);color:var(--alt)}.pill.ok{background:var(--ok-bg);color:var(--ok)}.pill.warn{background:var(--warn-bg);color:var(--warn)}.pill.bad{background:var(--bad-bg);color:var(--bad)}
.pill.active{border-color:currentColor}
.tablewrap{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:14px}
th{text-align:left;color:var(--mute);font-weight:500;font-size:12px;text-transform:uppercase;letter-spacing:.04em;padding:6px 8px;border-bottom:1px solid var(--line)}
td{padding:8px;border-bottom:1px solid var(--line);vertical-align:top}
tr.missing td{background:var(--bad-bg)}
tr.guess td{background:var(--warn-bg)}
tr.version td{background:var(--alt-bg)}
.tag.version{background:var(--alt);color:var(--card)}
tr.version .note{color:var(--alt)}
.tag{font-size:11px;font-weight:700;padding:2px 7px;border-radius:6px;white-space:nowrap}
.tag.found{background:var(--ok-bg);color:var(--ok)}.tag.guess{background:var(--warn);color:var(--card)}.tag.missing{background:var(--bad);color:#fff}
.path{font-size:12.5px;color:var(--mute);overflow-wrap:anywhere}
td{overflow-wrap:break-word}
table{table-layout:fixed;min-width:560px}
th:nth-child(1){width:34px}th:nth-child(2){width:40px}th:nth-child(3){width:28%}
td select{width:100%}
.fmt{font-size:11px;border:1px solid var(--line);border-radius:5px;padding:0 5px;color:var(--mute);margin-left:4px}
.note{font-size:12.5px;color:var(--warn)}
.sugg{font-size:12.5px;margin-top:4px}
.sugg a{color:var(--acc);cursor:pointer;text-decoration:underline;display:block}
.play{border:none;background:none;padding:0 4px;color:var(--acc);font-size:15px;cursor:pointer}
.bar{height:10px;background:var(--line);border-radius:6px;overflow:hidden;margin:10px 0}
.bar i{display:block;height:100%;background:var(--acc);width:0;transition:width .3s}
.banner{border-radius:10px;padding:12px 14px;margin-top:12px}
.banner.ok{background:var(--ok-bg);color:var(--ok)}.banner.bad{background:var(--bad-bg);color:var(--bad)}
.hidden{display:none!important}
.err{color:var(--bad);margin-top:8px}
</style></head><body><main>
<h1>Song Copy</h1>
<p class="sub">Give it a list of songs. It finds them in <b id="root">your music folder</b> and copies them to another folder.
It only copies, so nothing in your library is moved or deleted. <span id="libinfo" class="small"></span> <a href="#" id="rescan" class="small">Rescan library</a></p>

<section class="card">
  <div class="step"><b>1</b><h2>Your song list</h2></div>
  <div class="drop" id="drop"><strong>Drop a file here</strong> or click to choose<br>
  <span class="small">Spreadsheet (.csv, .xlsx), text file (one song per line), or playlist (.m3u)</span></div>
  <input type="file" id="file" class="hidden" accept=".csv,.tsv,.txt,.xlsx,.m3u,.m3u8">
  <details style="margin-top:10px"><summary class="small" style="cursor:pointer">…or paste a list</summary>
  <textarea id="paste" placeholder="Depeche Mode - World in My Eyes&#10;Robyn - Dancing On My Own&#10;Sigur Rós - Rembihnútur"></textarea>
  <div class="row" style="margin-top:8px"><button id="pastego">Find these songs</button></div></details>
  <div class="opts"><label>If a song exists as both FLAC and MP3, use
    <select id="prefer"><option value="flac">FLAC (best quality)</option><option value="mp3">MP3 (smaller, plays anywhere)</option><option value="any">Either one</option></select></label></div>
  <div id="loaderr" class="err"></div>
</section>

<section class="card hidden" id="results">
  <div class="step"><b>2</b><h2>Check what was found</h2></div>
  <div class="summary" id="summary"></div>
  <div class="row" style="margin-bottom:10px"><button id="dlmissing" class="hidden">⬇ Download the missing songs as a CSV</button>
    <span class="small" id="filterinfo"></span></div>
  <div class="tablewrap"><table><thead><tr><th style="width:28px"><input type="checkbox" id="all" checked title="select all"></th><th>#</th><th>On your list</th><th>File that will be copied</th></tr></thead>
  <tbody id="tbody"></tbody></table></div>
</section>

<section class="card hidden" id="copycard">
  <div class="step"><b>3</b><h2>Copy them to</h2></div>
  <p class="small" style="margin:-4px 0 10px">Songs already copied to this folder on an earlier run are hidden, so after adding missing songs to your library you can re-run the same list and only copy the new ones.</p>
  <div class="row"><input type="text" id="dest" placeholder="/home/you/Desktop/Party mix" spellcheck="false">
  <button id="browse">Browse…</button></div>
  <div class="browser" id="browser"></div>
  <div class="opts" style="color:var(--ink)">
    <label><input type="radio" name="mode" value="files" id="mode_files" checked> Copy the files as they are</label>
    <label><input type="radio" name="mode" value="usb" id="mode_usb"> USB stick for CDJs / XDJs</label>
  </div>
  <div class="opts" id="filesopts">
    <label><input type="checkbox" id="folders"> Keep Artist / Album folders</label>
    <label><input type="checkbox" id="number"> Number files in list order (001, 002…)</label>
    <label><input type="checkbox" id="playlist"> Also make a playlist file</label>
  </div>
  <div id="usbopts" class="hidden">
    <div class="opts"><label>Playlist folder on the stick <input type="text" id="usb_name" style="min-width:12em" spellcheck="false"></label>
      <label>Songs the decks can't play (FLAC, Opus…) become
      <select id="usb_format"><option value="aiff">AIFF (lossless)</option><option value="mp3">MP3 320 kbps (smaller)</option></select></label></div>
    <p class="small" style="margin:8px 0 0">Pick the USB stick (or a folder on it) above. Songs go into one folder, numbered in your list's order,
    and anything an older deck (XDJ-1000, CDJ-900/2000) can't play is converted; tags and artwork are kept. On the deck, browse the stick with its
    <b>folder</b> view. Use a stick formatted as <b>FAT32</b>, the one format every Pioneer deck reads.
    <span id="noffmpeg" class="hidden" style="color:var(--bad)"><br>ffmpeg isn't installed on this computer, so songs that need converting will fail. <span id="ffmpeghelp">Install ffmpeg, then restart Song Copy.</span></span></p>
  </div>
  <div class="row" style="margin-top:14px"><button class="primary" id="go">Copy songs</button><span class="small" id="gohint"></span></div>
  <div id="progress" class="hidden"><div class="bar"><i id="barfill"></i></div><div class="small" id="ptext"></div></div>
  <div id="done"></div>
</section>
<p class="small" id="ver" style="text-align:center;margin-top:24px"></p>
</main>
<audio id="audio"></audio>
<script>
const $=s=>document.querySelector(s);
let RESULTS=[], FILTER=null, LISTNAME='list';
const esc=s=>String(s??'').replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const kb=n=>n>1e9?(n/1073741824).toFixed(1)+' GB':(n/1048576).toFixed(1)+' MB';
async function api(p,body){const r=await fetch(p,body?{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:{});return r.json()}
const KEEP=['dest','prefer','folders','number','playlist','usb_format','mode_usb'];
try{for(const k of KEEP){const v=localStorage.getItem('sc_'+k);if(v==null)continue;const el=$('#'+k);(el.type==='checkbox'||el.type==='radio')?el.checked=v==='1':el.value=v}}catch(e){}
if($('#mode_usb').checked)$('#mode_files').checked=false;
function remember(){try{for(const k of KEEP){const el=$('#'+k);localStorage.setItem('sc_'+k,(el.type==='checkbox'||el.type==='radio')?(el.checked?'1':'0'):el.value)}}catch(e){}}
const usbMode=()=>$('#mode_usb').checked;
function showMode(){$('#filesopts').classList.toggle('hidden',usbMode());$('#usbopts').classList.toggle('hidden',!usbMode());if(RESULTS.length)render()}
document.querySelectorAll('input[name=mode]').forEach(r=>r.addEventListener('change',()=>{remember();showMode()}));
$('#usb_format').addEventListener('change',remember);
// songs already copied to the chosen folder (earlier runs) are hidden
let CHECKT=null;
async function checkCopied(){const dest=$('#dest').value.trim();const srcs=RESULTS.filter(r=>r.file).map(r=>r.file.path);
 if(!srcs.length)return;const res=dest?await api('/api/copied',{dest,srcs}):{copied:[]};const got=new Set(res.copied);
 RESULTS.forEach(r=>{const was=r.copied;r.copied=!!(r.file&&got.has(r.file.path));if(r.copied)r.checked=false;else if(was)r.checked=r.status==='found'||r.status==='guess'});render()}
$('#dest').addEventListener('input',()=>{clearTimeout(CHECKT);CHECKT=setTimeout(checkCopied,500)});
async function info(){const i=await api('/api/info');$('#root').textContent=i.root;
 if(!i.ready){$('#libinfo').textContent=`(reading your library for the first time… ${i.progress.toLocaleString()} songs so far)`;$('#rescan').classList.add('hidden')}
 else{$('#libinfo').textContent=`(${i.count.toLocaleString()} songs, checked ${i.scanned}${i.scanning?' · looking for new songs…':''})`;$('#rescan').classList.toggle('hidden',i.scanning)}
 if(!$('#dest').value&&i.dest){$('#dest').value=i.dest}$('#dest').placeholder=i.example;
 $('#noffmpeg').classList.toggle('hidden',!!i.ffmpeg);
 if(!i.ffmpeg&&i.windows)$('#ffmpeghelp').innerHTML=i.ffmpeg_installing?'Installing ffmpeg… (about a minute)':
   `<button id="getffmpeg">Install ffmpeg now</button> <span class="small">free, about a minute</span>`+(i.ffmpeg_log?`<br><span class="small">Last try: ${esc(i.ffmpeg_log)}</span>`:'');
 const gf=$('#getffmpeg');if(gf)gf.onclick=async()=>{await api('/api/install_ffmpeg',{});info()};
 if(i.ffmpeg_installing)setTimeout(info,2000);
 $('#ver').textContent='songcopy '+i.version;
 if(i.scanning)setTimeout(info,1000)}
info();
$('#rescan').onclick=async e=>{e.preventDefault();await api('/api/rescan',{});await info();const wait=async()=>{const i=await api('/api/info');if(i.scanning)return setTimeout(wait,1000);info();if(window.LAST)match(window.LAST)};wait()};
const drop=$('#drop');drop.onclick=()=>$('#file').click();
drop.ondragover=e=>{e.preventDefault();drop.classList.add('over')};drop.ondragleave=()=>drop.classList.remove('over');
drop.ondrop=e=>{e.preventDefault();drop.classList.remove('over');if(e.dataTransfer.files[0])loadFile(e.dataTransfer.files[0])};
$('#file').onchange=e=>{if(e.target.files[0])loadFile(e.target.files[0]);e.target.value=''};
$('#pastego').onclick=()=>{LISTNAME='pasted list';if(!$('#usb_name').value)$('#usb_name').value='My set';match({filename:'pasted.txt',text:$('#paste').value})};
$('#prefer').onchange=()=>{remember();if(window.LAST)match(window.LAST)};
function loadFile(f){LISTNAME=f.name.replace(/\.[^.]+$/,'');$('#usb_name').value=LISTNAME;const r=new FileReader();r.onload=()=>{const b=new Uint8Array(r.result);let s='';for(let i=0;i<b.length;i+=32768)s+=String.fromCharCode.apply(null,b.subarray(i,i+32768));match({filename:f.name,data:btoa(s)})};r.readAsArrayBuffer(f)}
async function match(req){window.LAST=req;$('#loaderr').textContent='';drop.innerHTML='<strong>Looking for your songs…</strong>'+($('#libinfo').textContent.includes('first time')?'<br><span class="small">(waiting for the library to finish reading - only slow the very first time)</span>':'');
 const res=await api('/api/match',{...req,prefer:$('#prefer').value});
 drop.innerHTML=`<strong>${esc(LISTNAME)}</strong> loaded. Drop another file to replace it.`;
 if(res.error){$('#loaderr').textContent=res.error;return}
 RESULTS=res.results.map(r=>({...r,checked:r.status==='found'||r.status==='guess'}));FILTER=RESULTS.some(r=>r.status==='missing')?null:null;render();
 $('#results').classList.remove('hidden');$('#copycard').classList.remove('hidden');$('#done').innerHTML='';$('#progress').classList.add('hidden');showMode();checkCopied()}
function fileCell(r,i){
 if(!r.file){let h=`<span class="tag missing">NOT FOUND</span>`;
  if(r.suggestions.length){h+=`<div class="sugg"><span class="small">Did you mean:</span>`+r.suggestions.map((s,k)=>`<a data-i="${i}" data-s="${k}">${esc(s.rel)}</a>`).join('')+`</div>`}
  h+=`<div class="sugg"><a data-search="${i}">Search the library myself…</a></div>`;return h}
 const f=r.file;let h=`<span class="tag ${r.status}">${{found:'FOUND',guess:'BEST GUESS',version:'OTHER VERSION ONLY'}[r.status]}</span> <b>${esc(f.title)}</b> <span class="small">by ${esc(f.artist)}</span><span class="fmt">${esc(f.ext.toUpperCase())}</span>`
 +` <button class="play" data-play="${esc(f.path)}" title="Listen">▶</button><div class="path">${esc(f.rel)}</div>`;
 if(r.note)h+=`<div class="note">${esc(r.note)}</div>`;
 if(r.alternatives.length){h+=`<div class="sugg"><select data-alt="${i}"><option value="">${r.alternatives.length} other version${r.alternatives.length>1?'s':''} in your library…</option>`+r.alternatives.map((a,k)=>`<option value="${k}">${esc(a.rel)}</option>`).join('')+`</select></div>`}
 return h}
function render(){
 const c={found:0,guess:0,version:0,missing:0,copied:0};RESULTS.forEach(r=>r.copied?c.copied++:c[r.status]++);
 $('#summary').innerHTML=(c.found||!c.copied?`<span class="pill ok ${FILTER==='found'?'active':''}" data-f="found">✓ ${c.found} ${c.copied?'new ':''}found</span>`:'')+
  (c.guess?`<span class="pill warn ${FILTER==='guess'?'active':''}" data-f="guess">? ${c.guess} best guess</span>`:'')+
  (c.version?`<span class="pill alt ${FILTER==='version'?'active':''}" data-f="version" title="You don't have the exact version asked for (e.g. Extended Mix). Not copied unless you tick it.">≠ ${c.version} other version only</span>`:'')+
  (c.missing?`<span class="pill bad ${FILTER==='missing'?'active':''}" data-f="missing">✕ ${c.missing} not found</span>`:'')+
  (c.copied?`<span class="pill ok ${FILTER==='copied'?'active':''}" data-f="copied" style="opacity:.75" title="Already copied to this folder on an earlier run. Click to see them (tick one to copy it again).">✓ ${c.copied} already copied</span>`:'')+
  `<span class="small" style="align-self:center">of ${RESULTS.length} songs on your list</span>`;
 $('#dlmissing').classList.toggle('hidden',!(c.missing||c.version));
 $('#filterinfo').textContent=FILTER==='copied'?'These were already copied to this folder. Tick one to copy it again.':FILTER?'Showing only one group. Click it again to show all.':(c.missing?'Songs that weren\'t found are shown in red. Click a suggestion to use it instead.':'')+(c.copied?' Already-copied songs are hidden.':'');
 const order={missing:0,guess:1,found:2};
 const rows=RESULTS.map((r,i)=>[r,i]).filter(([r])=>FILTER==='copied'?r.copied:(!r.copied&&(!FILTER||r.status===FILTER)));
 if(!rows.length&&!FILTER&&c.copied)$('#filterinfo').textContent='Everything found has already been copied to this folder. 🎉';
 $('#tbody').innerHTML=rows.map(([r,i])=>`<tr class="${r.status}"><td><input type="checkbox" data-c="${i}" ${r.checked?'checked':''} ${r.file?'':'disabled'}></td><td class="small">${r.row??''}</td><td>${esc(r.label)}</td><td>${fileCell(r,i)}</td></tr>`).join('');
 const sel=RESULTS.filter(r=>r.checked&&r.file);const bytes=sel.reduce((a,r)=>a+(r.file.size||0),0);
 $('#go').textContent=`${usbMode()?'Make USB with':'Copy'} ${sel.length} song${sel.length===1?'':'s'}`;$('#go').disabled=!sel.length;$('#gohint').textContent=sel.length?kb(bytes):'';
}
$('#summary').onclick=e=>{const f=e.target.closest('[data-f]');if(!f)return;FILTER=FILTER===f.dataset.f?null:f.dataset.f;render()};
$('#all').onchange=e=>{RESULTS.forEach(r=>{if(r.file&&(FILTER==='copied'?r.copied:(!r.copied&&(!FILTER||r.status===FILTER))))r.checked=e.target.checked});render()};
$('#tbody').onchange=e=>{const t=e.target;
 if(t.dataset.c!=null){RESULTS[t.dataset.c].checked=t.checked;render()}
 if(t.dataset.alt!=null&&t.value!==''){const r=RESULTS[t.dataset.alt];const a=r.alternatives.splice(+t.value,1)[0];r.alternatives.unshift(r.file);r.file=a;r.status='found';r.note='';r.checked=true;render()}};
$('#tbody').onclick=async e=>{const t=e.target;
 if(t.dataset.play){const au=$('#audio');const src='/api/play?path='+encodeURIComponent(t.dataset.play);if(au.dataset.p===t.dataset.play&&!au.paused){au.pause();t.textContent='▶';return}
  document.querySelectorAll('.play').forEach(b=>b.textContent='▶');au.src=src;au.dataset.p=t.dataset.play;au.play();t.textContent='❚❚';au.onended=()=>t.textContent='▶';return}
 if(t.dataset.s!=null){const r=RESULTS[t.dataset.i];r.file=r.suggestions[t.dataset.s];r.alternatives=r.suggestions.filter((_,k)=>k!=t.dataset.s);r.status='found';r.note='you picked this';r.checked=true;render();return}
 if(t.dataset.search!=null){const r=RESULTS[t.dataset.search];
  const cell=t.closest('td');cell.insertAdjacentHTML('beforeend',`<div class="row" style="margin-top:6px"><input type="text" placeholder="type artist or song words" value="${esc(r.label)}"><button>Search</button></div><div class="sugg"></div>`);
  t.remove();const inp=cell.querySelector('input[type=text]'),out=cell.querySelector('.sugg:last-child');
  const go=async()=>{const res=await api('/api/search',{q:inp.value,prefer:$('#prefer').value});r.suggestions=res.files;
   out.innerHTML=res.files.length?res.files.map((s,k)=>`<a data-i="${t.dataset.search}" data-s="${k}">${esc(s.rel)}</a>`).join(''):'<span class="small">Nothing found.</span>'};
  cell.querySelector('button').onclick=go;inp.onkeydown=e=>{if(e.key==='Enter')go()};inp.focus();go()}};
$('#dlmissing').onclick=()=>{const rows=[['Row','Song','Problem','What your library has instead']];RESULTS.filter(r=>r.status==='missing'||r.status==='version').forEach(r=>rows.push([r.row??'',r.label,r.status==='missing'?'not found':'only a different version',r.status==='missing'?(r.suggestions[0]?.rel||''):r.file.rel]));
 const csv=rows.map(r=>r.map(c=>'"'+String(c).replace(/"/g,'""')+'"').join(',')).join('\r\n');
 const a=document.createElement('a');a.href=URL.createObjectURL(new Blob(['﻿'+csv],{type:'text/csv'}));a.download=LISTNAME+' - missing songs.csv';a.click()};
// folder browser
async function ls(p){const r=await api('/api/ls?path='+encodeURIComponent(p));const b=$('#browser');b.style.display='block';
 b.innerHTML=(r.parent!=null?`<div data-p="${esc(r.parent)}">⬆ Up one level</div>`:'')+(r.path?`<div class="small" style="cursor:default">In <b>${esc(r.path)}</b> (${r.free} free) · click a folder to open it</div><div data-use="${esc(r.path)}" style="color:var(--acc)">✓ Copy into this folder</div>`:`<div class="small" style="cursor:default">This PC: pick a drive</div>`)+(r.error?`<div class="small" style="color:var(--bad)">${esc(r.error)}</div>`:'')+r.dirs.map(d=>`<div data-p="${esc(d.path)}">📁 ${esc(d.name)}</div>`).join('')}
$('#browse').onclick=()=>{const b=$('#browser');if(b.style.display==='block'){b.style.display='none';return}ls($('#dest').value||'~')};
$('#browser').onclick=e=>{const d=e.target.closest('div');if(!d)return;if(d.dataset.use){$('#dest').value=d.dataset.use;$('#browser').style.display='none';remember();checkCopied()}else if(d.dataset.p!=null){if(d.dataset.p)$('#dest').value=d.dataset.p;ls(d.dataset.p)}};
['dest','folders','number','playlist'].forEach(k=>$('#'+k).addEventListener('change',remember));
$('#go').onclick=async()=>{const dest=$('#dest').value.trim();if(!dest){$('#dest').focus();$('#gohint').textContent='Type or browse to a folder first.';return}
 const items=RESULTS.map((r,i)=>[r,i+1]).filter(([r])=>r.checked&&r.file).sort((a,b)=>a[1]-b[1]).map(([r,n])=>({src:r.file.path,rel:r.file.rel,n}));
 remember();$('#go').disabled=true;$('#done').innerHTML='';
 const res=await api('/api/copy',{dest,items,folders:$('#folders').checked,number:$('#number').checked,playlist:!usbMode()&&$('#playlist').checked,
  mode:usbMode()?'usb':'files',usb_format:$('#usb_format').value,usb_name:$('#usb_name').value});
 if(res.error){$('#done').innerHTML=`<div class="banner bad">${esc(res.error)}</div>`;$('#go').disabled=false;return}
 $('#progress').classList.remove('hidden');
 const tick=async()=>{const j=await api('/api/job/'+res.job);$('#barfill').style.width=(100*j.n/Math.max(1,j.total))+'%';
  $('#ptext').textContent=`${j.n} of ${j.total} songs${j.current?' · '+(usbMode()?'checking / converting ':'')+j.current:''}`;
  if(!j.finished)return setTimeout(tick,400);
  const miss=RESULTS.filter(r=>r.status==='missing'||(r.status==='version'&&!r.checked)).length;
  const doneSet=new Set(j.done_srcs);RESULTS.forEach(r=>{if(r.file&&doneSet.has(r.file.path)){r.copied=true;r.checked=false}});FILTER=null;render();
  let h=`<div class="banner ${j.errors.length?'bad':'ok'}"><b>Done.</b> ${j.copied+j.converted} copied to <b>${esc(j.dest)}</b>`+(j.converted?` (${j.converted} converted so the decks can play them)`:'')+(j.skipped?`, ${j.skipped} were already there (left alone)`:'')+'.'+(j.playlist?`<br>Playlist: ${esc(j.playlist)}`:'')+`<br><span class="small">Copied songs are now hidden from the list above.</span></div>`;
  if(j.errors.length)h+=`<div class="banner bad"><b>${j.errors.length} couldn't be copied:</b><br>`+j.errors.map(e=>esc(e.src)+'<br><span class="small">'+esc(e.msg)+'</span>').join('<br>')+'</div>';
  if(miss)h+=`<div class="banner bad">${miss} song${miss>1?'s were':' was'} not in your library (or only in a different version). Use the download button above to save the list.</div>`;
  $('#done').innerHTML=h;$('#go').disabled=false};tick()};
</script></body></html>"""

# ----------------------------------------------------------------------------- Windows: find the music server

# On Windows songcopy reads the music from the server's Samba share, \\<server>\home\Music.
# It finds the server by itself on any network: the name/address that worked last time ->
# a quick scan of this network -> asks you (and signs in to the share once).
# Settings (%LOCALAPPDATA%\songcopy\settings.json, all optional):
#   "server": name or address to try first (saved automatically, e.g. "myserver.local")
#   "share_music": the music folder on the server, below \\<server>\ (default "home\\Music")
SHARE_MUSIC_DEFAULT = "home\\Music"

def settings_file():
    base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~/.cache")
    return os.path.join(base, "songcopy", "settings.json")

def load_settings():
    try:
        with open(settings_file(), encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}

def save_settings(**kw):
    d = load_settings()
    d.update(kw)
    try:
        os.makedirs(os.path.dirname(settings_file()), exist_ok=True)
        with open(settings_file(), "w", encoding="utf-8") as fh:
            json.dump(d, fh, indent=1)
    except OSError:
        pass

def unc_music(host):
    share = load_settings().get("share_music") or SHARE_MUSIC_DEFAULT
    return "\\\\" + host + "\\" + share.strip("\\/").replace("/", "\\")

def port_open(host, port=445, timeout=0.6):
    import socket
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False

def local_networks():
    """the /24 networks this computer is on (e.g. ['192.168.1']) and its own addresses"""
    import socket
    ips = set()
    try:   # the address used to reach other networks (no packet is actually sent)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("10.254.254.254", 1))
            ips.add(s.getsockname()[0])
    except OSError:
        pass
    try:
        ips |= {a[4][0] for a in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET)}
    except OSError:
        pass
    nets = []
    for ip in sorted(ips):
        if ip.startswith(("127.", "169.254.")):
            continue
        n = ip.rsplit(".", 1)[0]
        if n not in nets:
            nets.append(n)
    return nets, ips

def find_server(say=print):
    """-> (host whose music share opens, or None;  best guess to sign in to)"""
    from concurrent.futures import ThreadPoolExecutor
    memo = load_settings().get("server")
    names = [h for h in dict.fromkeys([memo, os.environ.get("SONGCOPY_SERVER")]) if h]
    with ThreadPoolExecutor(8) as ex:
        reachable = [h for h, ok in zip(names, ex.map(port_open, names)) if ok]
    for h in reachable:
        if os.path.isdir(unc_music(h)):
            return h, h
    # reachable by NAME but not signed in yet: sign in by name, so it keeps working if the IP changes
    named = [h for h in reachable if not h[0].isdigit()]
    if named:
        return None, named[0]
    say("  Not at the usual address - scanning this network for the music server ...")
    nets, mine = local_networks()
    targets = [f"{n}.{i}" for n in nets for i in range(1, 255) if f"{n}.{i}" not in mine]
    with ThreadPoolExecutor(128) as ex:
        open_ = [h for h, ok in zip(targets, ex.map(lambda h: port_open(h, timeout=0.4), targets)) if ok]
    for h in open_:
        if os.path.isdir(unc_music(h)):
            return h, h
    return None, (reachable or open_ or [""])[0]

def windows_music_folder():
    """find (and if needed sign in to) the music on the server; returns \\\\host\\home\\Music"""
    import subprocess
    print(f"{C.D}Looking for your music server ...{C.X}")
    while True:
        host, guess = find_server()
        if host:
            save_settings(server=host)
            return unc_music(host)
        print(f"\n  {C.Y}Couldn't open the music on the server automatically.{C.X}")
        print("  That's normal the first time on a new network: Windows needs you to sign in once.")
        print("  Type the server's name (e.g. myserver.local, it keeps working if its address changes)")
        print("  or its address. On a Linux server,  hostname  shows the name and  hostname -I  the address.\n")
        try:
            addr = input(f"  Server name or address{f' [Enter = {guess}]' if guess else ''}: ").strip() or guess
        except EOFError:
            sys.exit("No server address given.")
        if not addr:
            continue
        addr = addr.strip("\\/ ")
        if not os.path.isdir(unc_music(addr)):
            print("\n  Sign in with your SERVER username and password (Windows will remember it):")
            subprocess.run(["net", "use", f"\\\\{addr}\\home", "/savecred", "/persistent:yes"])
        if os.path.isdir(unc_music(addr)):
            save_settings(server=addr)
            return unc_music(addr)
        print(f"\n  {C.R}Still can't open {unc_music(addr)}.{C.X} Check the address and that the server is on.")

def resolve_music(arg):
    if arg:
        return arg
    if WINDOWS:
        return windows_music_folder()
    return DEFAULT_MUSIC

# ----------------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(prog="songcopy", description="Copy a list of songs out of your music library (never moves or deletes).")
    ap.add_argument("--music", default=None,
                    help="music library folder (default: ~/Music; on Windows: the music server's share, found automatically)")
    ap.add_argument("--version", action="version", version=f"songcopy {VERSION}")
    sub = ap.add_subparsers(dest="cmd")
    s = sub.add_parser("serve", help="open the web page")
    s.add_argument("--port", type=int, default=8765)
    s.add_argument("--host", default="127.0.0.1", help="0.0.0.0 to allow other devices on your network")
    s.add_argument("--no-browser", action="store_true", help="don't open the browser automatically")
    s.add_argument("--dest", default=None, help="folder to suggest for copying into (e.g. your Traktor folder)")
    for name in ("copy", "find"):
        p = sub.add_parser(name, help="copy songs in a list" if name == "copy" else "check a list, copy nothing")
        p.add_argument("list", help="your list: .csv .tsv .xlsx .txt .m3u")
        if name == "copy":
            p.add_argument("dest", help="folder to copy into (created if needed)")
            p.add_argument("--folders", action="store_true", help="keep Artist/Album folders")
            p.add_argument("--number", action="store_true", help='prefix list order: "001 - "')
            p.add_argument("--playlist", action="store_true", help="also write playlist.m3u8")
            p.add_argument("--dry-run", "-n", action="store_true", help="show what would happen, copy nothing")
            p.add_argument("--usb", action="store_true",
                           help="make a USB for CDJs/XDJs: one numbered playlist folder; files decks can't play "
                                "(FLAC, Opus, ALAC, hi-res) are converted (needs ffmpeg)")
            p.add_argument("--usb-format", choices=["aiff", "mp3"], default="aiff",
                           help="what --usb converts to: aiff (lossless, default) or mp3 (320k, smaller)")
            p.add_argument("--name", help="playlist folder name for --usb (default: the list's file name)")
            p.add_argument("--recopy", action="store_true",
                           help="also copy songs that were already copied to DEST on an earlier run")
            p.add_argument("--any-version", action="store_true",
                           help="if the exact version (e.g. Extended Mix) isn't there, copy the version you have")
        p.add_argument("--prefer", choices=["flac", "mp3", "any"], default="flac")
        p.add_argument("--strict", action="store_true", help="don't copy best guesses")
    # let --music go after the subcommand too
    argv = sys.argv[1:]
    if "--music" in argv:
        i = argv.index("--music")
        argv = argv[i:i + 2] + argv[:i] + argv[i + 2:]
    # plain `songcopy` (maybe with serve options) = open the web page
    if not ({"serve", "copy", "find", "-h", "--help", "--version"} & set(argv)):
        argv.insert(2 if argv[:1] == ["--music"] else 0, "serve")
    args = ap.parse_args(argv)
    if args.cmd == "serve" and WINDOWS and not args.music and already_running(args.port):
        url = f"http://localhost:{args.port}"   # second double-click: just show the page again
        print(f"Song Copy is already running - opening {url}")
        open_browser(url)
        return
    args.music = resolve_music(args.music)
    if args.cmd == "serve":
        return serve(args)
    if not os.path.isdir(args.music):
        sys.exit(f"Music folder not found: {args.music}")
    sys.exit(cli_run(args, args.cmd == "copy"))

def run():
    """when the .exe is double-clicked, keep the window open on errors so they can be read"""
    if not (WINDOWS and getattr(sys, "frozen", False) and sys.stdin and sys.stdin.isatty()):
        return main()
    try:
        main()
    except SystemExit as e:
        if e.code not in (0, None):
            if not isinstance(e.code, int):
                print(e.code)
            input("\nPress Enter to close this window.")
        raise
    except Exception:
        import traceback
        traceback.print_exc()
        input("\nSomething went wrong (details above). Press Enter to close this window.")
        raise

if __name__ == "__main__":
    run()
