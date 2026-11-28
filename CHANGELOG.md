# Changelog

Each version's notes are shown in dubplate's update card before you install it.

## 1.0.0-alpha.3

**Test build.** Traktor comes to dubplate.

- **Traktor playlists as song lists.** Drop a playlist exported from Traktor (.nml), or your
  whole `collection.nml` and pick a playlist. Terminal: `--from-playlist NAME`.
- **Rekordbox sticks get Traktor's beat grid, BPM, key and cue points** for songs in your
  Traktor collection: hot cues 1–8 become A–H, other cues and loops memory cues, names kept.
  The collection is found in Traktor's usual folder, or chosen in **⚙ Settings → Traktor**.
  Check the cues on your decks before a gig.
- **Folders added under Settings → Also look in are really saved** (before, Save said
  "Nothing changed." and kept only the slskd key).
- **A yellow best guess that's wrong** has its own **🔎 Find on Soulseek** link.

## 1.0.0-alpha.2

**Test build.** Fixes found by a full bug check, and Soulseek downloads are found again.

- **Songs you download are found.** Files that aren't named "Artist - Album - 07 - Title"
  (slskd keeps the uploader's names, like `02. Kill for Love.flac`) are now recognised by
  their tags. Add slskd's download folder under **⚙ Settings → Also look in**, then Save.
- **Rekordbox sticks take more formats:** Ogg Vorbis directly; Opus, WMA and unusual WAV/AIFF
  through ffmpeg (named when it's missing). Damaged files are reported as damaged.
- Warnings before export for files the decks can't play as they are (Ogg, above 96 kHz/24-bit,
  very low sample rates) when you choose **leave them as they are**.
- A stick that is, holds or sits inside one of your library folders is refused.
- **Restore** also puts back files rbxport recovered, and never overwrites.
- Converted copies of songs are kept in dubplate's `decoded` folder; a song's new copy replaces
  its old one. Deleting that folder is always safe (it fills again when needed).
- **From another device** (phone, laptop) the page says what you can do there: updates,
  Settings, ffmpeg installs and rekordbox sticks only work on the computer dubplate runs on.
- The updater ignores a check made by another version, waits 30 minutes after a failed one,
  and never offers test builds to regular ones.
- Saving settings and other files can't be broken by a leftover temp file.
- Guides and help match what dubplate really does.

## 1.0.0-alpha.1

**Test build.** dubplate is in alpha until 1.0: builds are published as pre-releases and
only test builds offer them as updates. Version numbers start again at 1.0.0-alpha.1 (the
versions below were earlier development builds). Copies numbered 2.x don't find these versions by themselves:
download this one once by hand; it updates itself from then on.

**Rekordbox USB sticks, without rekordbox.** A third choice in step 3, **🎛 Rekordbox USB**,
makes a stick the decks read like one made in rekordbox:

- Your set is a **playlist in the deck's library browser**, in your list's order.
- **BPM, key, beat grid and waveforms** for every song. Each song is analysed once and
  remembered, so the next stick is quick.
- **Several sets per stick.** A new name adds a playlist, and the same name replaces it.
  Playlists already on the stick stay.
- Songs older decks can't play (FLAC, ALAC, AAC, Ogg, hi-res; Opus and WMA through ffmpeg) are **converted for the stick**:
  AIFF, MP3 320 or WAV. Or leave them as FLAC for decks that play it.
- The stick's library is **backed up before every export**. **Restore…** under the stick (or
  `dubplate restore-stick`) puts it back.

Terminal: `dubplate copy set.csv E:\ --rekordbox [--name NAME] [--convert aiff|mp3|wav|none]`.

Built on [rbxport](https://github.com/chrisle/rbxport)'s engine. It's tested on the CDJ-3000,
so **try a stick on your decks before a gig.**

## Earlier development builds

### 2.2.0

**Find missing songs on Soulseek.** If you use slskd (a Soulseek client), connect it in the
new **⚙ Settings**, then click **🔎 Find on Soulseek**:

- dubplate searches for the songs you don't have and **picks the best file for each**. It uses
  the same rules as for your library, so it **never picks a radio edit when you asked for the
  Extended Mix**. It prefers lossless, then MP3 320, skips anything under 192 kbps, and
  prefers people who can send it now.
- You **check the picks** (other choices are in a menu), then click Download. Nothing
  downloads until you do.
- Progress shows under each song. When the downloads finish, dubplate reads your library
  again and they turn green, ready to copy.

**⚙ Settings** (top of the page):

- **Main music folder**, and **Also look in** for other folders that count as your library:
  a second drive, or where your downloader saves songs.
- **Soulseek:** slskd's address and API key, with **Make a key for me** (it shows exactly
  what to add to slskd) and **Test connection**.

Settings can only be changed on the computer dubplate runs on. Your API key stays on your
computer.

### 2.1.1

**Making a USB after copying to your DJ folder now works the way you'd expect.** Step 3 is
two clear choices, **📁 Copy to a folder** and **💾 Make a USB for the decks**, each with its
own destination and its own memory. Songs you copied to your DJ folder still go on the stick.
Each playlist folder on a stick has its own memory too, so a song can be in several sets.

**Help everywhere.** Every option has a **?** that explains it (hover, or tap on a phone),
and **How dubplate works** at the top of the page is a short guide.

**A big round of bug fixes** (found by two full code reviews and by deliberately breaking
things):

- A copy cut off midway (stick full or pulled out, window closed) no longer leaves a broken
  song under its real name. Copies are written to a temporary file first and are only
  renamed once they're completely on the disk.
- Copying never replaces an existing file, even one created at the same moment.
- Very long file names (one in a 25,000-song library was 253 characters) no longer fail
  when numbered or put on a USB.
- USB without ffmpeg still copies MP3, WAV and AIFF instead of failing everything. With
  ffmpeg but no ffprobe, FLAC is still converted.
- Converted files are really written to the stick on Windows before they count as done.
- A different song with the same name and size is no longer mistaken for "already there",
  and re-runs don't create "(3)", "(4)"… copies.
- VLC-style playlists (`file:///…`) are read.
- macOS "._" leftover files aren't counted as songs; a folder that links back into itself
  no longer lists songs dozens of times. Band names starting with a dot (".38 Special") are
  kept.
- The page is safer when opened from other devices (`--host 0.0.0.0`): other websites can't
  use it, only songs from your library can be copied, and previews can't reach files outside
  it. New `--allow-host` option for a reverse-proxy domain.
- Updates: a failed update or "Go back" never loses the backup; "Go back" also works after
  updating from songcopy, and back again.
- Settings and the copy memory can no longer be corrupted by two windows saving at the same
  moment.
- Clearer error messages when a file can't be converted.

### 2.1.0

**songcopy is now called dubplate.** A dubplate is a one-off record cut just for a DJ to
play out, and that's what this makes: a one-off USB or folder of exactly your set, from your
own library. Nothing else changes.

- Your settings, the "already copied" memory and your saved song list come along by
  themselves. The old `songcopy` folder is left as it was.
- Updating from songcopy works as usual. The program file keeps whatever name you gave it;
  rename it to `dubplate.exe` if you like.
- Downloads are now `dubplate-windows-x86_64.exe` and `dubplate-linux-x86_64`.
- The project lives at github.com/vampyshiv/dubplate (old links still work).

### 2.0.2

- Fix: after **Update now** or **Go back**, songcopy on Linux could fail to restart by
  itself ("No such file or directory"). The update itself was installed; starting songcopy
  again by hand was needed. Windows had the same risk.

### 2.0.0

songcopy is now a native program, rewritten in Rust. One small download for Windows
(`songcopy-windows-x86_64.exe`) and Linux (`songcopy-linux-x86_64`), with nothing else
to install. It finds the same songs as before, about 4 times faster.

- **Updates built in.** The page tells you when a new version is out and what's in it.
  Click **Update now** and songcopy downloads it, checks it isn't damaged, swaps itself and
  restarts. **Go back to …** in the footer returns to the version you had. It checks at most
  every 6 hours; untick "check automatically" to turn that off. In a terminal:
  `songcopy update` (`--check`, `--rollback`).
- **Safer page.** Other websites open in your browser can no longer send requests to
  songcopy's page.
- Your settings, the "already copied" memory and the saved song list carry over.

**Windows:** if you used the old `songcopy.exe` (1.1), download this version once by hand.
From now on it updates itself.

### 1.1.0

- Remembers what was copied to each destination: re-running a list only copies the new finds.
- Files are numbered by their position in the list.
- USB stick for CDJs/XDJs: converts what older decks can't play (FLAC, Opus, ALAC, hi-res).
- Windows `songcopy.exe`.

### 1.0.0

- First version: web page and terminal, forgiving matching, version-aware (Extended Mix,
  Original Mix, remixes), missing-songs report.
