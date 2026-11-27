# songcopy

Copy a list of songs out of a big music library into another folder (a USB stick, a DJ
folder, a party mix) without hunting for each file by hand.

Give it a spreadsheet, a text list or a playlist. It finds each song in your library and
copies it, then tells you exactly which songs it couldn't find, so you know what to go
and get.

**It only ever copies.** It never moves, deletes or overwrites anything. If a file is
already in the destination it's skipped, and a different file with the same name gets
" (2)" added rather than being replaced.

One Python file, standard library only (Python 3.8+). Nothing to install.

## Using it

### Web page (easiest)

```
songcopy
```

This starts the tool and opens http://localhost:8765 in your browser.

1. **Drop in your list** (.csv, .xlsx, .txt, .m3u) or paste it. Rows pasted straight from
   Excel or Google Sheets work too.
2. **Check the results**, colour-coded:
   - **Green, found**: an exact match.
   - **Orange, best guess**: a close match (typo, odd spelling). Check it.
   - **Purple, other version only**: you asked for, say, the *Extended Mix* but only own
     the radio version or a remix. Shows what you have; **not copied unless you tick it**.
   - **Red, not found**: with "did you mean" suggestions and a search box.

   Every row has a ▶ button to listen first, and a menu to pick a different version.
   **Download the missing songs as a CSV** saves the red and purple rows.
3. **Choose a folder** and click **Copy**. Optional: keep Artist/Album folders, number the
   files in list order (001, 002…), write a playlist file.

### Terminal

```
songcopy find  mylist.csv                       # check only, copies nothing
songcopy copy  mylist.csv ~/Desktop/Party       # copy
songcopy copy  mylist.xlsx /media/usb --number --playlist --prefer mp3
```

| Option | What it does |
|---|---|
| `--prefer flac\|mp3\|any` | which format when a song exists in several (default `flac`) |
| `--folders` | keep Artist/Album folders instead of one flat folder |
| `--number` | put the list order in front of file names: `001 - …` |
| `--playlist` | also write `playlist.m3u8` |
| `--strict` | don't copy best guesses |
| `--any-version` | copy the version you have when the exact one (e.g. Extended Mix) isn't there |
| `--usb` | USB for CDJs/XDJs: numbered playlist folder, converts what decks can't play |
| `--usb-format aiff\|mp3` | what `--usb` converts to (default `aiff`) |
| `--name NAME` | playlist folder name for `--usb` (default: the list's file name) |
| `--recopy` | also copy songs already copied to that destination on an earlier run |
| `--dry-run` | show what would happen, copy nothing |
| `--music PATH` | the library (default `~/Music`) |
| `serve --no-browser --dest FOLDER --host 0.0.0.0 --port N` | web page options |

Songs it can't find are printed in red and saved next to your list as `<list>-missing.csv`.

### Re-running a list: only new songs

songcopy remembers what it copied to each destination. When you add the missing songs to
your library and run the same list again, songs already copied there are hidden (web page)
or skipped (terminal), so only the new finds get copied. A song is offered again if its
copy is deleted, or if you pick a different destination. Use `--recopy`, or tick it on the
page, to copy something again. The memory lives in `~/.cache/songcopy/copied.json`
(Windows: `%LOCALAPPDATA%\songcopy\`). Nothing extra is written into the destination.

### USB stick for CDJs / XDJs

Choose **USB stick for CDJs / XDJs** on the page (or `--usb` in the terminal) and pick the
stick.

- **One folder per playlist.** Songs are numbered in your list's order: `My set/001 - …`.
  The number is the song's position in the list, so songs you add later slot into their place.
- **Converted only where needed.** MP3, AAC and 16/24-bit WAV/AIFF at 44.1/48 kHz are
  copied unchanged. Everything older Pioneer decks (XDJ-1000, CDJ-900/2000) can't play,
  meaning FLAC, ALAC, Opus/Ogg/WMA and hi-res, is converted to **AIFF** (lossless, default)
  or **MP3 320**. Tags and artwork are kept.
- **On the deck:** browse the stick with the deck's folder view. Format the stick as
  **FAT32**, the one format every Pioneer deck reads.

Converting needs **ffmpeg**. On Windows the page has an **Install ffmpeg now** button
(uses winget). On Linux: `sudo apt install ffmpeg`.

This is a plain-folder USB, not a rekordbox database USB, so there are no rekordbox
playlists, waveforms or beat grids on the decks.

### Windows: songcopy.exe (e.g. straight into a Traktor folder)

The music stays on the server; songs are copied over the network into a folder on the
Windows PC. **Download `songcopy.exe`** from the latest
[release](../../releases) and double-click it. No Python needed. It:

- finds the server by itself on whatever network you're on: the name or address that worked
  last time, then a ~1 second scan of the network, then asks you. Type the server's
  name (like `myserver.local`) when asked, and it keeps working when the address changes,
- has you sign in to the share once (`net use`, remembered),
- opens the page. The folder picker shows the Windows drives.

It expects the music at `\\<server>\home\Music` (Samba share `home` = the server user's home
folder). Settings live in `%LOCALAPPDATA%\songcopy\settings.json`: `"server"` (name or
address to try first) and `"share_music"` (e.g. `"music"` for `\\<server>\music`). You can
always point it somewhere else with `songcopy.exe --music D:\Music`.

Windows may warn that the app is from an "unknown publisher" (it isn't code-signed):
click **More info → Run anyway**.

Moved the server to a new network? Make sure Samba (`hosts allow`) and the firewall allow
the new network's addresses. songcopy finds the new address by itself.

## How matching works

- **What it reads.** Names like `Artist - Album - 07 - Title.flac` in
  `Artist/Album (Year)/` folders, the usual Lidarr/beets/Picard style. Loose
  `Artist - Title.mp3` files work too.
- **Spreadsheet columns.** It recognises **Artist**, **Title** (or Song / Track / Name),
  **Album** and **File** (or Path), in any order. Spotify-export headers work. Lines
  without headers are read as `Artist - Title`, a filename, or a path.
- **Forgiving spelling.** Case, accents (`Sigur Ros` = `Sigur Rós`), punctuation, small
  typos, `- Remastered 2011` and `feat. X` don't matter.
- **Versions matter.**
  - `(Original Mix)`, `(Album Version)` and plain are the same.
  - `Extended Mix` = `Extended Version`.
  - `Club Mix`, `Radio Edit`, `Live` and `X Remix` are different versions.
- **Several copies.** When you own a song more than once, it prefers the original studio
  album over live recordings, compilations and remix albums, then your chosen format.
- **Long paths.** Paths over Windows' 260-character limit are handled.

The library is read in about a second for 25,000 songs. The song list is saved, so the
web page opens instantly and refreshes in the background, which matters over a network
share. The saved list lives in `~/.cache/songcopy/` (or `%LOCALAPPDATA%\songcopy\` on Windows).

## Development

```
python3 -m unittest discover -s tests
```

The tests build a small fake library in a temp folder, so they never touch real music.

**Windows exe.** Every push to `main` builds `songcopy.exe` on GitHub's Windows machines
(`.github/workflows/windows-exe.yml`) with PyInstaller. That build:
- runs the tests on Windows,
- smoke-tests the built `.exe` (including a path over 260 characters),
- keeps the `.exe` as a downloadable build artifact.

Pushing a version tag also publishes a release:

```
git tag v1.2.0 && git push origin v1.2.0
```

Bump `VERSION` in `songcopy.py` first.

## License

Copyright (C) 2026 vampyshiv. songcopy is free software: you can redistribute it and/or
modify it under the terms of the **GNU Affero General Public License, version 3 or (at your
option) any later version**. See [LICENSE](LICENSE).
