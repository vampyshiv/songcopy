# Changelog

Each version's notes are shown in dubplate's update card before you install it.

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
