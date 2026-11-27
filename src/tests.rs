//! The same tests as the Python version: a small fake library (empty files with real-looking
//! names) in a temp folder, so the tests never touch real music.

use crate::copy::{self, Item, Ledger};
use crate::library::{Library, MatchResult, Request};
use crate::paths::{self, join, TEST_ENV};
use std::sync::MutexGuard;

const LIBRARY: &[&str] = &[
    "Me/Depeche Mode/Violator (1990)/Depeche Mode - Violator - 06 - Enjoy the Silence.flac",
    "Me/Depeche Mode/Violator (1990)/Depeche Mode - Violator - 06 - Enjoy the Silence.mp3",
    "Me/Depeche Mode/Enjoy the Silence (1990)/Depeche Mode - Enjoy the Silence - 03 - Enjoy the Silence (Hands and Feet mix).flac",
    "Me/Depeche Mode/Touring the Angel - Live (2006)/Depeche Mode - Touring the Angel - Live - 03 - Enjoy the Silence.flac",
    "Me/Emptyset/Avichi EP (2011)/Emptyset - Avichi EP - 01 - Avichi (original mix).flac",
    "Me/Emptyset/Avichi EP (2011)/Emptyset - Avichi EP - 02 - Avichi (Monoloc remix).flac",
    "Me/HEALTH/GET COLOR (2009)/HEALTH - GET COLOR - 09 - IN VIOLET.flac",
    "Me/HEALTH/-DISCO2 (2010)/HEALTH - -DISCO2 - 08 - In Violet (Salem remix).flac",
    "Me/Robyn/Honey (Joe Goddard remix) (2019)/Robyn - Honey (Joe Goddard remix) - 02 - Honey (Joe Goddard remix).flac",
    "Me/Robyn/Robyn Is Here (1995)/Robyn - Robyn Is Here - 08 - Don't Want You Back.flac",
    "Me/Rammstein/Sehnsucht (1997)/Rammstein - Sehnsucht - 05 - Engel (extended version).flac",
    "Me/Sigur Rós/Valtari (2012)/Sigur Rós - Valtari - 04 - Rembihnútur.mp3",
    "Me/Silent Servant - Harm In Hand - Techno Cartel.mp3",
    "Podcasts/Some Show/Episode 1.mp3",
];

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    tmp: tempfile::TempDir,
    root: String,
    lib: Library,
}

fn fixture() -> Fixture {
    let guard = TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LOCALAPPDATA", tmp.path().join("appdata")); // keep songcopy's memory out of ~
    let root = tmp.path().join("Music").to_string_lossy().into_owned();
    for rel in LIBRARY {
        let p = join(&root, &rel.replace('/', &paths::SEP.to_string()));
        std::fs::create_dir_all(paths::dirname(&p)).unwrap();
        std::fs::write(&p, rel.as_bytes()).unwrap(); // unique content + size per file
    }
    let lib = Library::new(&root);
    lib.scan(|_| {});
    Fixture {
        _guard: guard,
        tmp,
        root,
        lib,
    }
}

fn m(f: &Fixture, artist: &str, title: &str) -> MatchResult {
    m_pref(f, artist, title, "flac")
}

fn m_pref(f: &Fixture, artist: &str, title: &str, prefer: &str) -> MatchResult {
    let req = Request {
        artist: artist.into(),
        title: title.into(),
        ..Default::default()
    };
    f.lib.view().match_one(&req, prefer)
}

fn assert_match(r: &MatchResult, status: &str, title: Option<&str>) {
    assert_eq!(r.status, status, "{r:?}");
    if let Some(t) = title {
        assert_eq!(r.file.as_ref().unwrap().title, t);
    }
}

#[test]
fn skips_podcasts_and_cache_round_trip() {
    let f = fixture();
    assert_eq!(f.lib.len(), LIBRARY.len() - 1);
    let lib2 = Library::new(&f.root);
    assert!(lib2.load_cache());
    assert_eq!(lib2.len(), f.lib.len());
}

#[test]
fn exact_prefers_studio_original_and_flac() {
    let f = fixture();
    let r = m(&f, "Depeche Mode", "Enjoy the Silence");
    assert_match(&r, "found", Some("Enjoy the Silence"));
    let file = r.file.unwrap();
    assert_eq!(file.ext, "flac");
    assert!(!file.rel.contains("Live"));
    assert_eq!(
        m_pref(&f, "Depeche Mode", "Enjoy the Silence", "mp3")
            .file
            .unwrap()
            .ext,
        "mp3"
    );
}

#[test]
fn sloppy_spelling_and_typos() {
    let f = fixture();
    assert_match(
        &m(&f, "sigur ros", "rembihnutur"),
        "found",
        Some("Rembihnútur"),
    );
    assert_match(
        &m(&f, "Robyn", "Don't Want You Back - Remastered 2011"),
        "found",
        Some("Don't Want You Back"),
    );
    assert_match(
        &m(&f, "Depeche Mode", "Enjoy the Slience"),
        "guess",
        Some("Enjoy the Silence"),
    );
}

#[test]
fn versions_and_remixes() {
    let f = fixture();
    assert_match(
        &m(&f, "Depeche Mode", "Enjoy the Silence - Hands and Feet Mix"),
        "found",
        Some("Enjoy the Silence (Hands and Feet mix)"),
    );
    assert_match(
        &m(&f, "HEALTH", "In Violet (Salem Remix)"),
        "found",
        Some("In Violet (Salem remix)"),
    );
    assert_match(
        &m(&f, "Emptyset", "Avichi"),
        "found",
        Some("Avichi (original mix)"),
    );
    assert_match(
        &m(&f, "Emptyset", "Avichi (Original Mix)"),
        "found",
        Some("Avichi (original mix)"),
    );
    assert_match(
        &m(&f, "Rammstein", "Engel - Extended Mix"),
        "found",
        Some("Engel (extended version)"),
    );
    // a version you don't own: offers the plain original first, not copied by default
    assert_match(
        &m(&f, "HEALTH", "In Violet (Extended Mix)"),
        "version",
        Some("IN VIOLET"),
    );
    assert_match(
        &m(&f, "Robyn", "Honey"),
        "version",
        Some("Honey (Joe Goddard remix)"),
    );
}

#[test]
fn missing_and_no_unrelated_suggestions() {
    let f = fixture();
    let r = m(&f, "Queen", "Bohemian Rhapsody");
    assert_eq!(r.status, "missing");
    assert!(r.file.is_none());
    assert!(m(&f, "Queen", "Bohemian Rhapsody (Extended Mix)")
        .suggestions
        .is_empty());
}

#[test]
fn free_text_and_filenames() {
    let f = fixture();
    let raw = |s: &str| {
        f.lib.view().match_one(
            &Request {
                raw: s.into(),
                ..Default::default()
            },
            "flac",
        )
    };
    assert_eq!(raw("Robyn - Don't Want You Back").status, "found");
    assert_eq!(
        raw("Silent Servant - Harm In Hand - Techno Cartel.mp3").status,
        "found"
    );
}

#[test]
fn never_overwrites_and_never_touches_the_source() {
    let f = fixture();
    let src = join(&f.root, &LIBRARY[0].replace('/', &paths::SEP.to_string()));
    let before = std::fs::read(&src).unwrap();
    let dest = f.tmp.path().join("dest").to_string_lossy().into_owned();
    let plan = copy::plan_copies(
        &[Item {
            src: src.clone(),
            rel: LIBRARY[0].into(),
            n: None,
        }],
        &dest,
        false,
        false,
    );
    let (s, d) = &plan[0];
    assert_eq!(copy::copy_one(s, d).0, "copied");
    assert_eq!(copy::copy_one(s, d).0, "skipped"); // same file again
    std::fs::write(d, b"something else").unwrap(); // different file, same name
    let (st, fin, _) = copy::copy_one(s, d);
    assert_eq!(st, "copied");
    assert!(fin.ends_with(" (2).flac"));
    assert_eq!(std::fs::read(d).unwrap(), b"something else"); // not overwritten
    assert_eq!(std::fs::read(&src).unwrap(), before); // library untouched
}

#[test]
fn numbering_name_clashes_and_safe_names() {
    let it = |s: &str, r: &str| Item {
        src: s.into(),
        rel: r.into(),
        n: None,
    };
    let items = [
        it("/a/x/01 - Intro.flac", "x/01 - Intro.flac"),
        it("/a/y/01 - Intro.flac", "y/01 - Intro.flac"),
        it("/a/x/01 - Intro.flac", "x/01 - Intro.flac"),
    ];
    let plan = copy::plan_copies(&items, "/dest", false, false);
    let names: Vec<&str> = plan.iter().map(|(_, d)| paths::basename(d)).collect();
    assert_eq!(names, ["01 - Intro.flac", "01 - Intro (2).flac"]);
    let plan = copy::plan_copies(&items[..2], "/dest", false, true);
    assert_eq!(paths::basename(&plan[0].1), "001 - 01 - Intro.flac");
    let plan = copy::plan_copies(
        &[Item {
            n: Some(7),
            ..it("/a/b.mp3", "")
        }],
        "/dest",
        false,
        true,
    );
    assert_eq!(paths::basename(&plan[0].1), "007 - b.mp3"); // its place in the list, not 001
    let plan = copy::plan_copies(&[it("/a/What?: A Song.mp3", "")], "/dest", false, false);
    assert_eq!(paths::basename(&plan[0].1), "What__ A Song.mp3");
}

#[test]
fn remembers_what_was_copied_per_destination() {
    let f = fixture();
    let src = join(&f.root, &LIBRARY[4].replace('/', &paths::SEP.to_string()));
    let dest = f.tmp.path().join("usb").to_string_lossy().into_owned();
    let other = f.tmp.path().join("other").to_string_lossy().into_owned();
    let plan = copy::plan_copies(
        &[Item {
            src: src.clone(),
            rel: LIBRARY[4].into(),
            n: Some(7),
        }],
        &join(&dest, "My set"),
        false,
        true,
    );
    let r = copy::transfer(&plan, &dest, None, |_, _| {});
    assert_eq!(r.copied, 1);
    let srcs = [src.clone()];
    assert!(Ledger::copied(&dest, &srcs).contains_key(&src));
    assert!(!Ledger::copied(&other, &srcs).contains_key(&src)); // other folder: not copied there
    std::fs::remove_file(&r.done[0]).unwrap();
    assert!(!Ledger::copied(&dest, &srcs).contains_key(&src)); // copy deleted: offer it again
                                                               // copied before the memory existed (or by hand): same name + size in the folder counts
    let plain = f.tmp.path().join("plain").to_string_lossy().into_owned();
    copy::copy_one(&src, &join(&plain, paths::basename(&src)));
    assert!(Ledger::copied(&plain, &srcs).contains_key(&src));
}

#[test]
fn usb_converts_what_decks_cant_play() {
    let (Some(ffmpeg), Some(_)) = copy::ffmpeg_path() else {
        eprintln!("ffmpeg not installed - skipping");
        return;
    };
    let f = fixture();
    let tone = |out: &str| {
        let ok = std::process::Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-metadata",
                "title=Test Tone",
                out,
            ])
            .status()
            .unwrap()
            .success();
        assert!(ok);
    };
    let dir = f.tmp.path().to_string_lossy().into_owned();
    let (flac, mp3) = (join(&dir, "a.flac"), join(&dir, "b.mp3"));
    tone(&flac);
    tone(&mp3);
    let usb = join(&join(&dir, "usb"), "set");
    let (st, fin, _) = copy::convert_one(&flac, &join(&usb, "001 - a.flac"), "aiff");
    assert_eq!((st, paths::splitext(&fin).1), ("converted", ".aiff"));
    let info = copy::probe(&fin).unwrap();
    assert!(copy::deck_playable(&fin, &info), "{info:?}");
    let (st, fin, _) = copy::convert_one(&mp3, &join(&usb, "002 - b.mp3"), "aiff");
    assert_eq!((st, paths::splitext(&fin).1), ("copied", ".mp3"));
    let (_, fin, _) = copy::convert_one(&flac, &join(&usb, "001 - a.flac"), "aiff"); // again: never overwrites
    assert!(fin.ends_with("001 - a (2).aiff"));
}

#[test]
fn what_decks_can_play() {
    let p = |codec: &str, rate, bits| copy::Probe {
        codec: codec.into(),
        rate,
        bits,
    };
    assert!(!copy::deck_playable("x.flac", &p("flac", 44100, 16)));
    assert!(!copy::deck_playable("x.wav", &p("pcm_s24le", 96000, 24)));
    assert!(copy::deck_playable("x.m4a", &p("aac", 44100, 0)));
    assert!(!copy::deck_playable("x.m4a", &p("alac", 44100, 16)));
}
