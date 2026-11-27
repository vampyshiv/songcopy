// dubplate - copy a list of songs out of a music library
// Copyright (C) 2026 vampyshiv
// SPDX-License-Identifier: AGPL-3.0-or-later
// This program is free software under the GNU Affero General Public License v3 or later; see LICENSE.

//! dubplate                          start + open the web page in your browser
//! dubplate serve [--port N]         same thing, explicitly (--no-browser, --dest FOLDER)
//! dubplate copy LIST DEST           copy every song in LIST into folder DEST
//! dubplate find LIST                just check which songs can be found (copies nothing)
//! dubplate update [--check]         install the newest version (--rollback: go back one)
//!
//! It only ever COPIES: it never moves, deletes or overwrites a file.

mod copy;
mod discover;
mod library;
mod lists;
mod paths;
mod rekordbox;
mod server;
mod slskd;
#[cfg(test)]
mod tests;
mod text;
mod update;

use clap::{Args, Parser, Subcommand};
use copy::{Item, Ledger};
use library::{Library, MatchResult};
use paths::{human, WINDOWS};
use std::io::{IsTerminal, Write};

#[derive(Parser)]
#[command(
    name = "dubplate",
    version,
    about = "Copy a list of songs out of your music library (never moves or deletes)."
)]
struct Cli {
    /// music library folder (default: ~/Music; on Windows: the music server's share, found automatically)
    #[arg(long, global = true)]
    music: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// open the web page
    Serve(ServeArgs),
    /// copy the songs in a list
    Copy(CopyArgs),
    /// check a list, copy nothing
    Find(FindArgs),
    /// put back a rekordbox stick's library as it was before an export
    /// (lists the stick's backups when no backup is named)
    RestoreStick {
        /// the stick (its top folder, e.g. E: or /media/you/USB)
        stick: String,
        /// which backup, as listed
        backup: Option<String>,
    },
    /// install the newest version of dubplate
    Update {
        /// only say whether there's a newer version
        #[arg(long)]
        check: bool,
        /// go back to the version you had before the last update
        #[arg(long)]
        rollback: bool,
    },
    /// (for testing) match requests read as JSON lines from stdin, print results as JSON lines
    #[command(hide = true)]
    MatchJson {
        #[arg(long, default_value = "flac")]
        prefer: String,
    },
}

#[derive(Args, Default)]
struct ServeArgs {
    #[arg(long, default_value_t = 8765)]
    port: u16,
    /// 0.0.0.0 to allow other devices on your network
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// don't open the browser automatically
    #[arg(long)]
    no_browser: bool,
    /// folder to suggest for copying into (e.g. your Traktor folder)
    #[arg(long)]
    dest: Option<String>,
    /// with --host 0.0.0.0: another name the page may be opened under (e.g. a reverse-proxy
    /// domain). Local names, IP addresses and this computer's own name always work.
    #[arg(long = "allow-host", value_name = "NAME")]
    allow_hosts: Vec<String>,
}

#[derive(Args)]
struct FindArgs {
    /// your list: .csv .tsv .xlsx .txt .m3u
    list: String,
    #[arg(long, value_parser = ["flac", "mp3", "any"], default_value = "flac")]
    prefer: String,
    /// don't copy best guesses
    #[arg(long)]
    strict: bool,
}

#[derive(Args)]
struct CopyArgs {
    /// your list: .csv .tsv .xlsx .txt .m3u
    list: String,
    /// folder to copy into (created if needed)
    dest: String,
    /// keep Artist/Album folders
    #[arg(long)]
    folders: bool,
    /// prefix list order: "001 - "
    #[arg(long)]
    number: bool,
    /// also write playlist.m3u8
    #[arg(long)]
    playlist: bool,
    /// show what would happen, copy nothing
    #[arg(long, short = 'n')]
    dry_run: bool,
    /// make a USB for CDJs/XDJs: one numbered playlist folder; files decks can't play
    /// (FLAC, Opus, ALAC, hi-res) are converted (needs ffmpeg)
    #[arg(long)]
    usb: bool,
    /// what --usb converts to: aiff (lossless) or mp3 (320k, smaller)
    #[arg(long, value_parser = ["aiff", "mp3"], default_value = "aiff")]
    usb_format: String,
    /// make a rekordbox USB: DEST is the stick; the list becomes a playlist in the decks'
    /// library browser, with beat grids, BPM, key and waveforms (no rekordbox needed).
    /// The stick's library is backed up first.
    #[arg(long, conflicts_with_all = ["usb", "folders", "number", "playlist"])]
    rekordbox: bool,
    /// what --rekordbox converts songs older decks can't play (FLAC, ALAC, Opus, ...) to;
    /// none = copy them as they are (CDJ-2000NXS2 / CDJ-3000 and newer play FLAC)
    #[arg(long, value_parser = ["aiff", "mp3", "wav", "none"], default_value = "aiff")]
    convert: String,
    /// playlist name for --usb / --rekordbox (default: the list's file name)
    #[arg(long)]
    name: Option<String>,
    /// also copy songs that were already copied to DEST on an earlier run
    #[arg(long)]
    recopy: bool,
    /// if the exact version (e.g. Extended Mix) isn't there, copy the version you have
    #[arg(long)]
    any_version: bool,
    #[arg(long, value_parser = ["flac", "mp3", "any"], default_value = "flac")]
    prefer: String,
    /// don't copy best guesses
    #[arg(long)]
    strict: bool,
}

struct C {
    r: &'static str,
    g: &'static str,
    y: &'static str,
    m: &'static str,
    b: &'static str,
    d: &'static str,
    x: &'static str,
    on: bool,
}

fn colours() -> C {
    let on = std::io::stdout().is_terminal();
    let c = |s: &'static str| if on { s } else { "" };
    C {
        r: c("\x1b[31m"),
        g: c("\x1b[32m"),
        y: c("\x1b[33m"),
        m: c("\x1b[35m"),
        b: c("\x1b[1m"),
        d: c("\x1b[2m"),
        x: c("\x1b[0m"),
        on,
    }
}

/// the main music folder: --music, else the one chosen in Settings, else (Windows) the music
/// server's share, else ~/Music
fn resolve_music(arg: Option<String>) -> String {
    if let Some(m) = arg {
        return m;
    }
    if let Some(m) = discover::setting_str("music").filter(|m| paths::isdir(m)) {
        return m;
    }
    if WINDOWS {
        discover::windows_music_folder()
    } else {
        paths::join(&paths::home(), "Music")
    }
}

fn main() {
    update::remember_exe();
    if WINDOWS && std::io::stdout().is_terminal() {
        // turn on colours in the old Windows console
        let _ = std::process::Command::new("cmd").args(["/C", ""]).status();
    }
    // plain `dubplate` (maybe with serve options) = open the web page
    let mut argv: Vec<String> = std::env::args().collect();
    let known = [
        "serve",
        "copy",
        "find",
        "update",
        "restore-stick",
        "match-json",
        "help",
        "-h",
        "--help",
        "-V",
        "--version",
    ];
    if !argv.iter().skip(1).any(|a| known.contains(&a.as_str())) {
        argv.insert(1, "serve".into());
    }
    let double_clicked = WINDOWS && argv.len() == 2 && std::io::stdin().is_terminal();
    let cli = Cli::parse_from(argv);
    let code = run(cli);
    if code != 0 && double_clicked {
        // keep the window open so the message can be read
        print!("\nPress Enter to close this window.");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    std::process::exit(code);
}

fn run(cli: Cli) -> i32 {
    let cmd = cli.cmd.unwrap_or(Cmd::Serve(ServeArgs {
        port: 8765,
        host: "127.0.0.1".into(),
        ..Default::default()
    }));
    match cmd {
        Cmd::Serve(a) => {
            let restarting = crate::update::is_restart();
            if WINDOWS && cli.music.is_none() && !restarting && server::already_running(a.port) {
                let url = format!("http://localhost:{}", a.port); // second double-click: just show the page again
                println!("Dubplate is already running - opening {url}");
                server::open_browser(&url);
                return 0;
            }
            let music_from_cli = cli.music.is_some();
            let music = resolve_music(cli.music);
            server::serve(server::Options {
                music_from_cli,
                music,
                port: a.port,
                host: a.host,
                no_browser: a.no_browser,
                dest: a.dest,
                allow_hosts: a.allow_hosts,
            });
            0
        }
        Cmd::Update { check, rollback } => update_cmd(check, rollback),
        Cmd::RestoreStick { stick, backup } => {
            let stick = rekordbox::stick_path(&stick);
            let all = rekordbox::list_backups(&stick);
            match backup {
                None if all.is_empty() => {
                    println!("No backups of {stick} yet (one is made before every rekordbox export to it).");
                    0
                }
                None => {
                    println!("Backups of {stick}, newest first:");
                    for b in &all {
                        println!("  {b}");
                    }
                    // quoted for the shell: Windows double quotes (a trailing \ doubled so it doesn't
                    // escape the quote), elsewhere single quotes
                    let shown = if WINDOWS {
                        format!("\"{}{}\"", stick, if stick.ends_with('\\') { "\\" } else { "" })
                    } else {
                        format!("'{}'", stick.replace('\'', "'\\''"))
                    };
                    println!("Put one back with: dubplate restore-stick {shown} \"<backup>\"");
                    0
                }
                Some(b) => match rekordbox::restore(&stick, &b) {
                    Ok(r) => {
                        println!("Put back the stick's library from {b}.");
                        if let Some(p) = &r.problem {
                            eprintln!("But {p}. Try the stick on a deck before a gig, or restore another backup.");
                        }
                        if let Some(n) = r.backup {
                            println!("What it held just now is backed up too ({n}).");
                        }
                        if !r.missing.is_empty() {
                            println!("{} song(s) in the restored playlists were taken off the stick by a later export; send those sets again to put them back:", r.missing.len());
                            for m in r.missing.iter().take(20) {
                                println!("  {m}");
                            }
                        }
                        i32::from(r.problem.is_some())
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        1
                    }
                },
            }
        }
        Cmd::MatchJson { prefer } => match_json(&resolve_music(cli.music), &prefer),
        Cmd::Find(a) => {
            let music = resolve_music(cli.music);
            cli_run(&music, &a.list, &a.prefer, a.strict, None)
        }
        Cmd::Copy(a) => {
            let music = resolve_music(cli.music);
            let (list, prefer, strict) = (a.list.clone(), a.prefer.clone(), a.strict);
            cli_run(&music, &list, &prefer, strict, Some(a))
        }
    }
}

fn match_json(music: &str, prefer: &str) -> i32 {
    use std::io::BufRead;
    let lib = Library::new(music);
    lib.set_extra(&discover::setting_list("extra_folders"));
    lib.scan(|_| {});
    let view = lib.view();
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if let Ok(req) = serde_json::from_str::<library::Request>(&line) {
            let _ = writeln!(
                out,
                "{}",
                serde_json::to_string(&view.match_one(&req, prefer)).unwrap_or_default()
            );
        }
    }
    0
}

fn update_cmd(check: bool, rollback: bool) -> i32 {
    let c = colours();
    if rollback {
        return match update::rollback() {
            Ok(v) => {
                println!("{}Went back to dubplate {v}.{}", c.g, c.x);
                0
            }
            Err(e) => {
                eprintln!("{}{e}{}", c.r, c.x);
                1
            }
        };
    }
    if let Err(e) = update::check_now() {
        eprintln!("{}Couldn't check for updates: {e}{}", c.r, c.x);
        return 1;
    }
    let st = serde_json::to_value(update::status(false)).unwrap_or_default();
    let latest = st["latest"].as_str().unwrap_or("?");
    if !st["available"].as_bool().unwrap_or(false) {
        println!("You have the newest version ({}).", update::VERSION);
        return 0;
    }
    println!(
        "{}dubplate {latest} is available{} (you have {})\n",
        c.b,
        c.x,
        update::VERSION
    );
    if let Some(n) = st["notes"].as_str().filter(|n| !n.is_empty()) {
        println!("{n}\n");
    }
    if check {
        println!("Run  dubplate update  to install it.");
        return 0;
    }
    match update::apply() {
        Ok(v) => {
            println!(
                "{}Updated to dubplate {v}.{} (dubplate update --rollback goes back)",
                c.g, c.x
            );
            0
        }
        Err(e) => {
            eprintln!(
                "{}Update failed: {e}{} - you still have {}.",
                c.r,
                c.x,
                update::VERSION
            );
            1
        }
    }
}

fn cli_run(music: &str, list: &str, prefer: &str, strict: bool, copy: Option<CopyArgs>) -> i32 {
    let c = colours();
    if !paths::isfile(list) {
        eprintln!("{}Can't find the list file: {list}{}", c.r, c.x);
        return 1;
    }
    if !paths::isdir(&paths::abspath(&paths::expanduser(music))) {
        eprintln!("{}Music folder not found: {music}{}", c.r, c.x);
        return 1;
    }
    let lib = Library::new(music);
    lib.set_extra(&discover::setting_list("extra_folders")); // "also look in" folders from Settings
                                                             // every folder searched: the main one, plus "also look in" folders from Settings
    let folders = lib.roots().join(" + ");
    print!("{}Reading your library {} ...{}", c.d, folders, c.x);
    let _ = std::io::stdout().flush();
    let t0 = std::time::Instant::now();
    let n = lib.scan(|k| {
        if c.on && k % 2000 == 0 {
            print!(
                "\r{}Reading your library ... {} songs{}",
                c.d,
                paths::thousands(k),
                c.x
            );
            let _ = std::io::stdout().flush();
        }
    });
    println!(
        "\r{}Read your library {}: {} songs ({:.1}s){}        ",
        c.d,
        folders,
        paths::thousands(n),
        t0.elapsed().as_secs_f64(),
        c.x
    );
    let data = match std::fs::read(list) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}{e}{}", c.r, c.x);
            return 1;
        }
    };
    let reqs = match lists::read_rows(list, &data) {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => {
            eprintln!("{}No songs found in {list}{}", c.r, c.x);
            return 1;
        }
        Err(e) => {
            eprintln!("{}{e}{}", c.r, c.x);
            return 1;
        }
    };
    println!(
        "{}Looking up {} songs from {} ...{}\n",
        c.d,
        reqs.len(),
        paths::basename(list),
        c.x
    );
    let view = lib.view();
    let results: Vec<MatchResult> = reqs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut m = view.match_one(r, prefer);
            m.n = i + 1;
            m
        })
        .collect();
    let by = |s: &str| results.iter().filter(|r| r.status == s).collect::<Vec<_>>();
    let (found, guess, missing, other) = (by("found"), by("guess"), by("missing"), by("version"));
    let any_version = copy.as_ref().is_some_and(|a| a.any_version);
    let row = |r: &MatchResult| match &r.row {
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    };
    let rel = |r: &MatchResult| r.file.as_ref().map(|f| f.rel.clone()).unwrap_or_default();

    if copy.is_none() && !found.is_empty() {
        println!("{}{}Found ({}):{}", c.g, c.b, found.len(), c.x);
        for r in &found {
            let extra = if r.alternatives.is_empty() {
                String::new()
            } else {
                format!("  (+{} other versions)", r.alternatives.len())
            };
            println!(
                "  {}row {:>4}{}  {}\n            {}→ {}{extra}{}",
                c.g,
                row(r),
                c.x,
                r.label,
                c.d,
                rel(r),
                c.x
            );
        }
        println!();
    }
    if !guess.is_empty() {
        println!(
            "{}{}Best guesses ({}) - not exact, please check:{}",
            c.y,
            c.b,
            guess.len(),
            c.x
        );
        for r in &guess {
            println!(
                "  {}row {:>4}{}  {}\n            {}→ {}{}",
                c.y,
                row(r),
                c.x,
                r.label,
                c.d,
                rel(r),
                c.x
            );
        }
        let how = if strict {
            "skipped because of --strict"
        } else {
            "these WILL be copied; use --strict to skip them"
        };
        println!("  {}({how}){}\n", c.d, c.x);
    }
    if !other.is_empty() {
        println!(
            "{}{}Only a DIFFERENT VERSION in your library ({}):{}",
            c.m,
            c.b,
            other.len(),
            c.x
        );
        for r in &other {
            println!(
                "  {}row {:>4}  {}{}\n            {}you have: {}{}",
                c.m,
                row(r),
                r.label,
                c.x,
                c.d,
                rel(r),
                c.x
            );
        }
        let how = if any_version {
            "these WILL be copied (--any-version)"
        } else {
            "NOT copied; add --any-version to copy what you have instead"
        };
        println!("  {}({how}){}\n", c.d, c.x);
    }
    if !missing.is_empty() {
        println!(
            "{}{}NOT FOUND in {} ({}):{}",
            c.r,
            c.b,
            folders,
            missing.len(),
            c.x
        );
        for r in &missing {
            println!("  {}row {:>4}  {}{}", c.r, row(r), r.label, c.x);
            if let Some(s) = r.suggestions.first() {
                println!("            {}closest: {}{}", c.d, s.rel, c.x);
            }
        }
        println!();
    }
    let other_txt = if other.is_empty() {
        String::new()
    } else {
        format!("{}{} other-version-only{}{}, ", c.m, other.len(), c.x, c.b)
    };
    println!(
        "{}Found {} exact, {} best-guess, {other_txt}{}{} missing{}{} (of {}){}",
        c.b,
        found.len(),
        guess.len(),
        if missing.is_empty() { "" } else { c.r },
        missing.len(),
        c.x,
        c.b,
        results.len(),
        c.x
    );
    let mut missing_csv = None;
    if !missing.is_empty() || !other.is_empty() {
        let p = format!("{}-missing.csv", paths::splitext(&paths::abspath(list)).0);
        if copy::write_missing_csv(&p, &results).is_ok() {
            println!(
                "Missing{} list saved to: {}{p}{}",
                if other.is_empty() {
                    ""
                } else {
                    " / other-version"
                },
                c.b,
                c.x
            );
            missing_csv = Some(p);
        }
    }
    let incomplete = i32::from(!missing.is_empty() || !other.is_empty());
    let Some(a) = copy else { return incomplete };

    let mut to_copy: Vec<&MatchResult> = found.to_vec();
    if !strict {
        to_copy.extend(guess.iter().copied());
    }
    if any_version {
        to_copy.extend(other.iter().copied());
    }
    to_copy.sort_by_key(|r| r.n);
    let mut items: Vec<Item> = to_copy
        .iter()
        .filter_map(|r| {
            r.file.as_ref().map(|f| Item {
                src: f.path.clone(),
                rel: f.rel.clone(),
                n: Some(r.n),
            })
        })
        .collect();

    let dest = if a.rekordbox { rekordbox::stick_path(&a.dest) } else { paths::abspath(&paths::expanduser(&a.dest)) };
    if a.rekordbox {
        let stem = paths::splitext(paths::basename(list)).0.to_string();
        let name = a.name.clone().unwrap_or(stem);
        let songs: Vec<rekordbox::Song> = to_copy
            .iter()
            .filter_map(|r| {
                r.file.as_ref().map(|f| rekordbox::Song {
                    path: f.path.clone(),
                    artist: f.artist.clone(),
                    title: f.title.clone(),
                    album: f.album.clone(),
                })
            })
            .collect();
        if songs.is_empty() {
            println!("{}Nothing to put on the stick.{}", c.y, c.x);
            return incomplete;
        }
        println!("\nMaking the rekordbox playlist {}{name}{} on {}{dest}{} ({} songs)", c.b, c.x, c.b, c.x, songs.len());
        if a.dry_run {
            println!("{}(dry run: nothing was written){}", c.d, c.x);
            return incomplete;
        }
        let on = c.on;
        let r = rekordbox::export_set(&dest, &name, &songs, rekordbox::parse_convert(&a.convert), &|p| {
            if on {
                let count = if p.total > 0 { format!("{}/{}", p.done, p.total) } else { String::new() };
                print!("\r  {:<10} {count:<9} {:<50}", p.stage, p.current.chars().take(50).collect::<String>());
                let _ = std::io::stdout().flush();
            }
        });
        if c.on {
            print!("\r{}\r", " ".repeat(80));
        }
        return match r {
            Ok(r) => {
                println!(
                    "{}{}Done: {} songs in the playlist {}{}{} ({} copied, {} already on the stick, {} converted){}",
                    c.g, c.b, r.tracks, r.playlist, c.x, c.g, r.copied, r.reused, r.converted, c.x
                );
                if let Some(b) = &r.backup {
                    println!("{}The stick's library from before is backed up ({b}); `dubplate restore-stick` puts it back.{}", c.d, c.x);
                }
                if !r.verified {
                    println!("{}The stick couldn't be checked after writing.{}", c.y, c.x);
                }
                for w in &r.warnings {
                    println!("{}  {w}{}", c.y, c.x);
                }
                if !r.errors.is_empty() {
                    println!("{}{}{} left out:{}", c.r, c.b, r.errors.len(), c.x);
                    for (p, e) in &r.errors {
                        println!("  {}{} {e}{}", c.r, paths::basename(p), c.x);
                    }
                    return 1;
                }
                incomplete
            }
            Err(e) => {
                eprintln!("{}{e}{}", c.r, c.x);
                1
            }
        };
    }
    let usb_format = a.usb.then(|| a.usb_format.clone());
    // a USB stick remembers per playlist folder; a folder remembers itself
    let usb_target = usb_format.as_ref().map(|_| {
        let stem = paths::splitext(paths::basename(list)).0.to_string();
        copy::usb_folder(&dest, a.name.as_deref().unwrap_or(&stem))
    });
    if !a.recopy {
        let srcs: Vec<String> = items.iter().map(|i| i.src.clone()).collect();
        let before = match &usb_target {
            Some(t) => Ledger::copied_usb(&dest, t, &srcs),
            None => Ledger::copied(&dest, &srcs),
        };
        if !before.is_empty() {
            items.retain(|i| !before.contains_key(&i.src));
            println!(
                "{}{} already copied to {} on an earlier run - skipped{} {}(--recopy to copy them again){}",
                c.g,
                before.len(),
                usb_target.as_deref().unwrap_or(&dest),
                c.x,
                c.d,
                c.x
            );
            if items.is_empty() {
                println!("{}{}Nothing new to copy.{}", c.g, c.b, c.x);
                return incomplete;
            }
        }
    }
    let (target, plan) = if let Some(t) = usb_target.clone() {
        let (ff, fp) = copy::ffmpeg_path();
        if ff.is_none() {
            println!("{}ffmpeg isn't installed: songs the decks can't play (FLAC etc.) can't be converted; MP3, WAV and AIFF are copied. Install ffmpeg first.{}", c.y, c.x);
        } else if fp.is_none() {
            println!("{}ffprobe (part of ffmpeg) isn't installed: files can't be checked, so they're judged by their type.{}", c.y, c.x);
        }
        let p = copy::plan_copies(&items, &t, false, true);
        (t, p)
    } else {
        (
            dest.clone(),
            copy::plan_copies(&items, &dest, a.folders, a.number),
        )
    };
    let total = copy::estimate_size(&plan, usb_format.as_deref());
    let verb = format!(
        "{}{}",
        if a.dry_run { "Would copy" } else { "Copying" },
        usb_format
            .as_ref()
            .map(|f| format!(
                " (for decks: anything they can't play → {})",
                f.to_uppercase()
            ))
            .unwrap_or_default()
    );
    println!(
        "\n{verb} {} files (about {}) to {}{target}{}",
        plan.len(),
        human(total),
        c.b,
        c.x
    );
    let free = server::free_space(&paths::nearest_existing(&dest));
    if total > free {
        eprintln!(
            "{}Not enough space: need about {}, only {} free on that drive.{}",
            c.r,
            human(total),
            human(free),
            c.x
        );
        return 1;
    }
    if a.dry_run {
        for (_, d) in plan.iter().take(20) {
            println!("  {}{}{}", c.d, paths::relpath(d, &target), c.x);
        }
        if plan.len() > 20 {
            println!("  {}... and {} more{}", c.d, plan.len() - 20, c.x);
        }
        println!("{}(dry run: nothing was copied){}", c.d, c.x);
        return incomplete;
    }
    let total_n = plan.len();
    let r = copy::transfer(&plan, &target, usb_format.as_deref(), |k, s| {
        if c.on {
            let name: String = paths::basename(s).chars().take(60).collect();
            print!("\r  {k}/{total_n}  {name:<60}");
            let _ = std::io::stdout().flush();
        }
    });
    if c.on {
        print!("\r{}\r", " ".repeat(80));
    }
    if a.playlist && !r.done.is_empty() {
        if let Ok(p) = copy::write_playlist(&target, &r.done) {
            println!("Playlist: {p}");
        }
    }
    println!(
        "{}{}Done: {} copied{}{}{}",
        c.g,
        c.b,
        r.copied + r.converted,
        c.x,
        if r.converted > 0 {
            format!(" ({} converted for the decks)", r.converted)
        } else {
            String::new()
        },
        if r.skipped > 0 {
            format!(", {} already there (skipped)", r.skipped)
        } else {
            String::new()
        }
    );
    if r.unchecked > 0 {
        println!(
            "{}{} WAV/AIFF file{} copied without checking (ffprobe isn't installed): older decks only play them at 44.1/48 kHz, 16/24-bit.{}",
            c.y,
            r.unchecked,
            if r.unchecked > 1 { "s were" } else { " was" },
            c.x
        );
    }
    if !r.errors.is_empty() {
        println!(
            "{}{}{} could not be copied:{}",
            c.r,
            c.b,
            r.errors.len(),
            c.x
        );
        for e in &r.errors {
            println!("  {}{}\n    {}{}", c.r, e.src, e.msg, c.x);
        }
    }
    if !missing.is_empty() {
        let n = missing.len();
        println!(
            "{}{n} song{} not in your library - see {}{}",
            c.r,
            if n > 1 { "s were" } else { " was" },
            missing_csv.unwrap_or_default(),
            c.x
        );
    }
    i32::from(!missing.is_empty() || !r.errors.is_empty())
}
