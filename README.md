# dubplate

*(formerly songcopy)*

Copy a list of songs out of a big music library into another folder (a USB stick, a DJ
folder, a party mix) without hunting for each file by hand.

Give it a spreadsheet, a text list or a playlist. It finds each song in your library and
copies it, then tells you exactly which songs it couldn't find, so you know what to go
and get.

**It only ever copies.** It never moves, deletes or overwrites anything. If a file is
already in the destination it's skipped, and a different file with the same name gets
" (2)" added rather than being replaced. (The one place dubplate manages files is the library
on a [rekordbox stick](#rekordbox-usb-sticks), as rekordbox does; it's backed up first.)

It can also make **rekordbox USB sticks** for Pioneer decks, with playlists, BPM, key, beat
grids and waveforms, without rekordbox.

A single small program for Windows and Linux, written in Rust. Nothing else to install,
and it updates itself.

**New to dubplate?** The [guides](docs/index.html) walk you through everything step by
step, for DJs of every level.

## Contents

[Install](#install) · [Quick start](#quick-start) · [Using the web page](#using-the-web-page) ·
[Terminal](#terminal) · [USB sticks for CDJs/XDJs](#usb-sticks-for-cdjs--xdjs) ·
[Rekordbox USB sticks](#rekordbox-usb-sticks) ·
[Missing songs on Soulseek](#finding-missing-songs-on-soulseek) · [Settings](#settings) ·
[What it remembers](#what-it-remembers) · [Windows](#windows-music-on-a-home-server) ·
[How matching works](#how-matching-works) · [Troubleshooting](#troubleshooting) ·
[Development](#development)

## Install

> **dubplate is in alpha.** Until 1.0, builds are test builds, published as
> [pre-releases](../../releases). Expect rough edges, and try anything for a gig beforehand.

Download the program for your computer from the [releases page](../../releases) (the newest
at the top):

- **Windows:** `dubplate-windows-x86_64.exe`. Put it wherever you like (the desktop is
  fine) and double-click it. Windows may warn about an "unknown publisher" because the app
  isn't code-signed: click **More info → Run anyway**.
- **Linux:** `dubplate-linux-x86_64`. It runs on any 64-bit Linux:

  ```
  install -m 755 dubplate-linux-x86_64 ~/.local/bin/dubplate
  ```

### Updates

The page tells you when a new version is out and shows what changed. Click **Update now**:
dubplate downloads it, checks it against the release's checksum, keeps the current version
and restarts. **Go back to …** in the page footer returns to the version you had.

- It asks GitHub for the newest release at most every 6 hours, and nothing else is sent.
- Untick **check automatically** to turn that off; **Check for updates** still works.
- In a terminal: `dubplate update`, `dubplate update --check` or `dubplate update --rollback`.

## Quick start

1. Run `dubplate` (or double-click it on Windows). Your browser opens its page.
2. Drop your song list onto the page.
3. Check what it found (green = found, orange = check it, purple = only another version,
   red = missing).
4. Pick **Copy to a folder** (e.g. your Traktor folder) or **Make a USB for the decks**, choose
   where, and click the button.
5. Missing songs: if you use slskd, **🔎 Find on Soulseek** fetches them (see
   [below](#finding-missing-songs-on-soulseek)). Otherwise get them your own way, then load the
   same list again: only the new ones are copied.

Every option on the page has a **?** next to it that explains it (hover, or tap on a phone),
and **How dubplate works** at the top of the page is a short guide.

## Using the web page

```
dubplate
```

This starts dubplate and opens http://localhost:8765 in your browser. Keep the window it
runs in open while you use the page; close it to stop. Running it again while it's already
running just opens the page.

### 1. Your song list

Drop a file onto the page, or click to choose one, or paste the list:

- **Spreadsheets** (.csv, .tsv, .xlsx): a header row with **Artist**, **Title** (or Song /
  Track / Name), **Album** and **File** (or Path), in any order. Spotify exports (e.g.
  from Exportify) work as they are.
- **Text files** (.txt), one song per line: `Artist - Title`, a file name, or a path.
- **Playlists** (.m3u / .m3u8), including VLC-style `file:///…` entries and playlists made on
  another computer (matched by name).
- **Pasted** rows from Excel or Google Sheets, or `Artist - Title` lines.

**If a song exists as both FLAC and MP3** picks which copy to use when you own both.

### 2. Check what was found

| Colour | Means | Copied? |
|---|---|---|
| **Green**, found | the exact song | yes |
| **Orange**, best guess | a close match: a typo or a different spelling. Listen with ▶ to check | yes (untick to leave it out) |
| **Purple**, other version only | you asked for e.g. the *Extended Mix* but only have the radio edit or a remix | **no**, unless you tick it |
| **Red**, not found | not in your library; "Did you mean" suggestions and a search box help | no |

- Click a coloured count to show only those songs.
- Every song has a **▶** button to listen, and a menu listing the other versions you own.
- **Download the missing songs as a CSV** saves the red and purple songs, with the closest
  thing your library has: a shopping list.

### 3. Where should they go?

Three choices, each with its own destination, remembered separately:

- **📁 Copy to a folder.** For your DJ software (e.g. the folder Traktor watches) or any
  folder. Files are copied exactly as they are. Options:
  - **Keep Artist / Album folders**: recreate your library's folders. Off: one flat folder.
  - **Number files in list order**: `007 - …`, so the folder sorts in your list's order.
  - **Also make a playlist file**: `playlist.m3u8`, for importing into DJ or music apps.
- **💾 Make a USB for the decks.** A plain-folder stick. See [USB sticks](#usb-sticks-for-cdjs--xdjs).
- **🎛 Rekordbox USB.** A stick the decks read like one made in rekordbox. See
  [Rekordbox USB sticks](#rekordbox-usb-sticks).

Songs already copied to the chosen place are hidden; see [What it remembers](#what-it-remembers).

## Terminal

```
dubplate find  mylist.csv                       # check only, copies nothing
dubplate copy  mylist.csv ~/Desktop/Party       # copy
dubplate copy  mylist.xlsx /media/usb --number --playlist --prefer mp3
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
| `--rekordbox` | make a [rekordbox stick](#rekordbox-usb-sticks): DEST is the stick, the list becomes a playlist |
| `--convert aiff\|mp3\|wav\|none` | what `--rekordbox` converts songs older decks can't play to (default `aiff`) |
| `--name NAME` | playlist (folder) name for `--usb` / `--rekordbox` (default: the list's file name) |
| `--recopy` | also copy songs already copied to that destination on an earlier run |
| `--dry-run` | show what would happen, copy nothing |
| `--music PATH` | the library (default `~/Music`; on Windows the server's share) |
| `serve --no-browser --dest FOLDER --port N` | web page options |
| `serve --host 0.0.0.0 [--allow-host NAME]` | open the page from other devices; see [below](#opening-the-page-from-another-device) |

Songs it can't find are printed in red and saved next to your list as `<list>-missing.csv`.

```
dubplate copy friday.csv E:\ --rekordbox             # a rekordbox playlist "friday" on the stick
dubplate restore-stick E:                             # list the stick's backups
dubplate restore-stick E: "2026-11-27 21.04.10"       # put one back
```

## USB sticks for CDJs / XDJs

Choose **💾 Make a USB for the decks** on the page (or `--usb` in the terminal) and pick the
stick, or a folder on it.

- **One folder per set.** Songs go into the **playlist folder** you name (`Friday/001 - …`),
  numbered in your list's order. The number is the song's position in the list, so songs you
  add later slot into their place. Use a different folder name for each set; several sets can
  live side by side on one stick.
- **Converted only where needed.** MP3, AAC and 16/24-bit WAV/AIFF at 44.1/48 kHz are
  copied unchanged. Everything older Pioneer decks (XDJ-1000, CDJ-900/2000) can't play,
  meaning FLAC, ALAC, Opus/Ogg/WMA and hi-res, is converted to **AIFF** (lossless, default)
  or **MP3 320**. Tags and artwork are kept.
- **On the deck:** browse the stick with the deck's folder view. Format the stick as
  **FAT32**, the one format every Pioneer deck reads.

Converting needs **ffmpeg**. On Windows the page has an **Install ffmpeg now** button
(uses winget). On Linux: `sudo apt install ffmpeg`.

If ffmpeg isn't installed, files that need converting can't be copied, but MP3, WAV and AIFF
still are. With no ffprobe to check them, dubplate tells you how many WAV/AIFF files went
over unchecked, since older decks only play those at 44.1/48 kHz, 16/24-bit.

This is a plain-folder USB: the decks show folders, with no BPM, waveforms or beat grids.
For those, make a [rekordbox stick](#rekordbox-usb-sticks) instead.

Copies are written to a hidden temporary file and only get their real name once they're
completely on the stick. A stick pulled out, or a window closed mid-copy, never leaves a
half-written song behind under its real name.

## Rekordbox USB sticks

Choose **🎛 Rekordbox USB** on the page (or `--rekordbox` in the terminal) and pick the
stick itself (its top folder, e.g. `E:\` or `/media/you/USB`).

The decks then read the stick as if rekordbox had made it:

- **Your set is a playlist** in the deck's library browser (under PLAYLIST), in your list's
  order.
- **BPM, key, beat grid and waveforms** for every song: the overview and scrolling waveforms,
  in colour on decks that draw colour.
- **Several sets per stick.** A new playlist name adds a playlist next to the others. Sending
  a set with the same name again replaces that playlist. Playlists already on the stick stay,
  including ones made on a deck or by rekordbox.
- **Songs older decks can't play are converted** for the stick: FLAC, ALAC, Ogg/Opus and
  hi-res become AIFF (default), MP3 320 or WAV. Your library's files are never changed. Pick
  "Leave them as they are" for decks that play FLAC (CDJ-2000NXS2, CDJ-3000 and newer).
  No ffmpeg needed.
- Use a **FAT32** stick (exFAT works on newer decks).

**Analysis.** Each song is analysed once, the way rekordbox's Normal analysis does it: one
tempo fitted to the whole song, then the beat grid, key and waveforms. That takes a few
seconds a song, several songs at a time. The result is remembered, so the next stick with
that song is quick. A song is analysed again only when its file changes. A file that can't
be read is left out and listed; the rest of the set still goes on.

**Backups.** Before anything is written, dubplate keeps a copy of the stick's library (its
databases, not the music). The page lists them under the stick, each with **Restore…**;
in the terminal, `dubplate restore-stick STICK`. Restore puts the stick's playlists back as
they were, and first backs up what the stick holds now.

- An export that changes nothing, or that fails before writing anything, doesn't use up a
  backup.
- dubplate keeps the **first** backup (the library from before dubplate first touched the
  stick) plus the newest ones, 10 in all. A backup you restore is never pushed out by the one
  made just before it.
- Backups belong to the stick, not to the drive letter it gets today. dubplate leaves a small
  folder on it, `.dubplate`, with the stick's id. On Windows and Linux it also notes the
  stick's serial number, so a stick copied from another one gets backups of its own.
- Only one dubplate writes a stick at a time; a second one (another window, a terminal) is
  told to wait.
- Restoring is only possible on the computer dubplate runs on, not from another device.

**Managing the stick, as rekordbox does.** If a song is in no playlist on the stick any more
(you re-sent a set without it), its copy on the stick is removed with the next export. These
are only files dubplate put on the stick, never your library. A restore brings that set's
playlist back, but not the removed copy: dubplate says which songs are missing, so you can
send that set again.

**Safe if interrupted.** The stick's databases are swapped in only once everything is
written, by rbxport for an export and by dubplate for a restore. A stick pulled out, or
dubplate closed in the middle, leaves the stick as it was before, or the job is finished
next time dubplate touches the stick.

**Which decks?** The stick format is the one rekordbox writes for Pioneer CDJ/XDJ decks
(`export.pdb`, plus `exportLibrary.db` for the newest). rbxport's author tests it on a
CDJ-3000. **Try a stick on your own decks before a gig.**

This is built on [rbxport](https://github.com/chrisle/rbxport)'s engine by Chris Le (see
[License](#license)).

## Finding missing songs on Soulseek

If you run [slskd](https://github.com/slskd/slskd) (a Soulseek client with a web page),
dubplate can look for your missing songs there and download the ones you pick.

**Connect it once** (⚙ Settings → Soulseek):

1. **slskd address:** the address you open slskd with, e.g. `http://localhost:5030`.
2. **API key:** click **Make a key for me**. dubplate shows these lines with your new key:

   ```yaml
   web:
     authentication:
       api_keys:
         dubplate:
           key: <your key>
           role: readwrite
   ```

   Add them to slskd's options: in slskd's web page under **System → Options → Edit**, or in
   its `slskd.yml`. If there's already an `api_keys:` line, add only the `dubplate:` part
   under it. Save, and restart slskd if it asks. The key needs the role `readwrite`: slskd's
   default (read-only) can't search or download.
3. Click **Test connection**. It shows slskd's version and whether it's logged in to Soulseek.
4. **Add slskd's download folder** under *Also look in*, so downloaded songs are found.
   dubplate offers it with one click when it can see that folder. If slskd runs in Docker,
   its folders have other names inside the container than on your computer: pick the same
   folder with **+ Add a folder…** instead.
5. **Save settings.**

**Use it:** after loading a list, click **🔎 Find N on Soulseek** (or **Find on Soulseek**
under one song). For each song, dubplate:

- searches with the song's words (artist, title, and the version, e.g. "extended"; without
  noise like "(Original Mix)" or "feat. X"),
- reads the results with the same rules it uses for your library, so a **radio edit is never
  picked when you asked for the Extended Mix**,
- prefers lossless (FLAC/AIFF/WAV), then MP3 320, drops anything under 192 kbps, and then
  prefers people who can send it now,
- shows its pick under the song, ticked if it's an exact match, with the other results in a
  menu.

Check the picks, then click **⬇ Download N from Soulseek**. Progress shows under each song
("waiting in queue", "downloading 45%"). When they're done, dubplate reads your library
again and the songs turn green, ready to copy.

dubplate only searches and queues what you confirm; slskd does the downloading, with its
own settings and limits. Your API key is stored only on your computer, in dubplate's
settings file, and is never shown again after saving.

## Settings

**⚙ Settings** is at the top of the page:

- **Main music folder:** the folder with your music. On Windows the default is the music
  server's share, found automatically.
- **Also look in:** other folders with songs that should count as your library, such as a
  second drive or the folder your downloader saves to. Songs from them show the folder's
  name in front of their path.
- **Soulseek (slskd):** address and API key; see [above](#finding-missing-songs-on-soulseek).

Settings can only be changed on the computer dubplate runs on, not from another device. The
terminal commands use the same folders (`--music` replaces the main one for that run).

## What it remembers

dubplate remembers what it copied **to each destination**: each folder, and each playlist
folder on a stick.

- **Load the same list again later** (after getting the missing songs) and only the new ones
  are offered: songs already copied there are hidden on the page (click **✓ already copied**
  to see them) and skipped in the terminal.
- **A different destination has its own memory.** Songs you copied to your DJ folder still
  go on a USB stick, and a set on the stick doesn't stop the same song going into another set.
- A song is offered again if its copy has been deleted. To copy something again anyway,
  tick it on the page, or use `--recopy`.
- Nothing extra is written into your destinations (apart from a rekordbox stick's own
  library and dubplate's `.dubplate` folder on it). The memory, settings, saved song analysis and a saved copy of
  your library's song list live in `~/.cache/dubplate/` (Windows:
  `%LOCALAPPDATA%\dubplate\`). Deleting that folder resets dubplate.
- Rekordbox stick backups are kept in `~/.local/share/dubplate/stick-backups/` (Windows:
  `%LOCALAPPDATA%\dubplate\stick-backups\`), not in a cache folder, so cleaners leave them be.
- Settings and memory from **songcopy** (dubplate's old name) are brought over the first time
  dubplate runs; the old folder is left in place.

## Windows: music on a home server

The music stays on the server; songs are copied over the network into a folder on the
Windows PC. Double-click `dubplate-windows-x86_64.exe` (see [Install](#install)). It:

- finds the server by itself on whatever network you're on: the name or address that worked
  last time, then a ~1 second scan of the network, then asks you. Type the server's
  name (like `myserver.local`) when asked, and it keeps working when the address changes,
- has you sign in to the share once (`net use`, remembered),
- opens the page. The folder picker shows the Windows drives.

It expects the music at `\\<server>\home\Music` (Samba share `home` = the server user's home
folder). Settings live in `%LOCALAPPDATA%\dubplate\settings.json`: `"server"` (name or
address to try first) and `"share_music"` (e.g. `"music"` for `\\<server>\music`). You can
always point it somewhere else with `dubplate.exe --music D:\Music`.

Windows may warn that the app is from an "unknown publisher" (it isn't code-signed):
click **More info → Run anyway**.

Moved the server to a new network? Make sure Samba (`hosts allow`) and the firewall allow
the new network's addresses. dubplate finds the new address by itself.

## Opening the page from another device

By default the page only opens on the computer running dubplate. To open it from your phone
or laptop too, start it with `dubplate serve --host 0.0.0.0` and visit
`http://<that computer's name or address>:8765`.

- Anyone on your network can then use it to copy songs from your library (never delete them).
- **Only your songs can be copied from the network**, not other files on that computer.
- The page answers when opened by this computer's address, its name
  (`myserver`, `myserver.local`, `myserver.fritz.box`…) and local names (`.local`, `.lan`,
  `.home`, `.internal`). It refuses public websites' names, so a website can't trick your
  browser into using dubplate. Behind a reverse proxy with its own domain, allow that name
  with `--allow-host dubplate.example.com`.

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

The library is read in about a second for 25,000 songs, and a list of 1,000 songs is
matched in well under a second. The song list is saved, so the
web page opens instantly and refreshes in the background, which matters over a network
share. The saved list lives in `~/.cache/dubplate/` (or `%LOCALAPPDATA%\dubplate\` on Windows).

## Troubleshooting

- **"Windows protected your PC".** The app isn't code-signed. Click **More info → Run
  anyway**; you only see this for the first download, not for updates.
- **A song is red but you have it.** Click **Search the library myself…** under it, or check
  the spelling against your file names. Matching reads file and folder names, not tags.
  Then click **Rescan library** if you added it just now.
- **The deck doesn't show the rekordbox playlist.** Pick the stick's top folder (`E:\`), not
  a folder on it; use a FAT32 stick; eject it properly before unplugging. Choose the stick as
  the deck's source and open **PLAYLIST**, not the folder view.
- **A BPM is half or double.** The analysis fits one tempo to the whole song, the way
  rekordbox's Normal analysis does, and a song that's mostly half-time can read as half the
  tempo. dubplate can't correct a BPM by hand yet.
- **"couldn't write the stick".** The message says why; common causes are a full stick or one
  that's write-protected. If anything on the stick had changed, the message names the backup
  that holds the library from before.
- **A song is purple.** You don't have the exact version asked for (e.g. *Extended Mix*).
  Tick it to copy the version you do have, or pick another from its menu.
- **Songs I expected are hidden.** They were already copied to that destination. Click
  **✓ already copied** to see them and tick any to copy again.
- **The deck doesn't show the songs.** Use the deck's folder view (not its library/rekordbox
  view), and a stick formatted as FAT32.
- **"couldn't convert it - the file may be damaged".** ffmpeg couldn't read that file; try
  playing it on the computer. Its message is included.
- **"ffmpeg isn't installed".** On Windows click **Install ffmpeg now** on the page. On
  Linux: `sudo apt install ffmpeg`.
- **Windows can't find the server.** Make sure you can open `\\<server>\home\Music` in File
  Explorer. On the server, Samba's `hosts allow` and the firewall must allow your network.
- **Port 8765 is taken.** `dubplate serve --port 8766`.
- **Soulseek: "didn't accept the API key".** The key in ⚙ Settings must match the one in
  slskd's options exactly, and slskd may need a restart after adding it.
- **Soulseek: "the API key is read-only".** Give the key `role: readwrite` in slskd.
- **Soulseek: downloaded, but still red.** The song is in slskd's download folder, which
  isn't in your library folders: add it under ⚙ Settings → Also look in.
- **Soulseek: "Nothing on Soulseek right now".** Nobody online shares it at the moment, or
  it's named differently; try again later or edit the song's name on your list.

## Development

You need [Rust](https://rustup.rs). Then:

```
cargo test             # unit tests, on a fake library in a temp folder (never real music)
cargo build --release  # target/release/dubplate
```

- **Code layout** (`src/`):
  - `library.rs`: reading the library and matching.
  - `text.rs`: name normalisation, version tags, and a port of Python's difflib.
  - `lists.rs`: reading lists.
  - `copy.rs`: copying, USB conversion, the copy memory.
  - `server.rs` and `page.html`: the web page.
  - `discover.rs`: settings and the Windows server finder.
  - `slskd.rs`: Soulseek through slskd (search, ranking, downloads), with a fake slskd for tests.
  - `update.rs`: the updater.
- **Builds.** Every push builds and tests the Windows and Linux programs on GitHub's machines
  (`.github/workflows/build.yml`), including a smoke test of each built program
  (`tests/smoke_exe.py`, with a path over Windows' 260-character limit).
- **Releases.** Bump the version in `Cargo.toml`, add its notes to `CHANGELOG.md` (the
  updater shows them), then:

  ```
  git tag v1.0.0-alpha.2 && git push origin v1.0.0-alpha.2
  ```

  The release gets both programs and a `SHA256SUMS` file the updater checks downloads against.
  Alpha/beta tags (anything with a `-`) are published as **pre-releases**: only alpha/beta
  copies of dubplate are offered them. A plain tag (`v1.0.0`) is a regular release, offered
  to everyone.

Versions before 2.0 were a single Python script; it's in the git history.

## License

Copyright (C) 2026 vampyshiv. dubplate is free software: you can redistribute it and/or
modify it under the terms of the **GNU Affero General Public License, version 3 or (at your
option) any later version**. See [LICENSE](LICENSE).

The rekordbox stick engine comes from [rbxport](https://github.com/chrisle/rbxport)
(copyright Chris Le and contributors, GNU General Public License version 2 or later). dubplate
uses its `rbl-*` crates unchanged, pinned to release v1.10.0. dubplate's analysis code is
adapted from rbxport's app. Both licences let the combined program be distributed under the
AGPL-3.0-or-later.
