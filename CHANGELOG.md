# Changelog

Each version's notes are shown in dubplate's update card before you install it.

## 2.1.1

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

## 2.1.0

**songcopy is now called dubplate.** A dubplate is a one-off record cut just for a DJ to
play out, and that's what this makes: a one-off USB or folder of exactly your set, from your
own library. Nothing else changes.

- Your settings, the "already copied" memory and your saved song list come along by
  themselves. The old `songcopy` folder is left as it was.
- Updating from songcopy works as usual. The program file keeps whatever name you gave it;
  rename it to `dubplate.exe` if you like.
- Downloads are now `dubplate-windows-x86_64.exe` and `dubplate-linux-x86_64`.
- The project lives at github.com/vampyshiv/dubplate (old links still work).

## 2.0.2

- Fix: after **Update now** or **Go back**, songcopy on Linux could fail to restart by
  itself ("No such file or directory"). The update itself was installed; starting songcopy
  again by hand was needed. Windows had the same risk.

## 2.0.0

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

## 1.1.0

- Remembers what was copied to each destination: re-running a list only copies the new finds.
- Files are numbered by their position in the list.
- USB stick for CDJs/XDJs: converts what older decks can't play (FLAC, Opus, ALAC, hi-res).
- Windows `songcopy.exe`.

## 1.0.0

- First version: web page and terminal, forgiving matching, version-aware (Extended Mix,
  Original Mix, remixes), missing-songs report.
