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
| `--dry-run` | show what would happen, copy nothing |
| `--music PATH` | the library (default `~/Music`) |
| `serve --no-browser --dest FOLDER --host 0.0.0.0 --port N` | web page options |

Songs it can't find are printed in red and saved next to your list as `<list>-missing.csv`.

### From a Windows PC (e.g. straight into a Traktor folder)

The music stays on the server; the songs are copied over the network into a folder on the
Windows PC.

1. Share your home folder from the server with Samba (as `\\<server>\home`), with this
   project at `~/songcopy`.
2. On Windows, open `\\<server>\home\songcopy` in File Explorer and double-click
   **`Song Copy (Windows).bat`**. It:
   - installs Python if it's missing (one keypress, via winget),
   - finds the server by itself on whatever network you're on: the address that worked
     last time, then `dahlias-den.local`, then a ~1 second scan of the network, then asks you,
   - has you sign in to the share once (remembered),
   - puts a **Song Copy** shortcut on the desktop,
   - opens the page. The folder picker shows the Windows drives.

To use a different server name, edit `dahlias-den` in the `.bat`. To fill in the
destination automatically, set `TRAKTOR=` near the top.

Moved the server to a new network? Make sure Samba (`hosts allow`) and the firewall allow
the new network's addresses. The launcher finds the new address by itself.

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
