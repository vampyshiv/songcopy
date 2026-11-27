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
    std::env::set_var("LOCALAPPDATA", tmp.path().join("appdata")); // keep dubplate's memory out of ~
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
    let f = fixture(); // takes the environment lock before looking for ffmpeg
    let (Some(ffmpeg), Some(_)) = copy::ffmpeg_path() else {
        eprintln!("ffmpeg not installed - skipping");
        return;
    };
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

#[test]
fn brings_songcopy_settings_and_memory_along() {
    let _g = TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_string_lossy().into_owned();
    std::env::set_var("LOCALAPPDATA", &base);
    let old = join(&base, "songcopy");
    std::fs::create_dir_all(join(&old, "previous")).unwrap();
    std::fs::write(
        join(&old, "settings.json"),
        br#"{"server": "myserver.local"}"#,
    )
    .unwrap();
    std::fs::write(join(&old, "copied.json"), b"{}").unwrap();
    let dir = paths::app_dir();
    assert!(dir.ends_with("dubplate"));
    assert_eq!(
        crate::discover::setting_str("server").as_deref(),
        Some("myserver.local")
    );
    assert!(paths::isfile(&join(&dir, "copied.json")));
    assert!(paths::isfile(&join(&old, "settings.json"))); // the old folder is left as it was
    std::fs::write(join(&old, "settings.json"), br#"{"server": "changed"}"#).unwrap();
    assert_eq!(
        crate::discover::setting_str("server").as_deref(),
        Some("myserver.local")
    ); // only once
}

// ---------------------------------------------------------------------------- bug-hunt regressions

#[test]
fn copies_go_through_a_temp_file_and_never_overwrite() {
    let f = fixture();
    let src = join(&f.root, &LIBRARY[0].replace('/', &paths::SEP.to_string()));
    let dest = f.tmp.path().join("t").to_string_lossy().into_owned();
    let dst = join(&dest, "x.flac");
    let (st, fin, _) = copy::copy_one(&src, &dst);
    assert_eq!((st, fin.as_str()), ("copied", dst.as_str()));
    let leftovers: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left behind");
    // finishing a temp file onto a name that appeared meanwhile picks a new name instead
    let part = copy::part_name(&dst);
    std::fs::write(&part, b"new").unwrap();
    let fin = copy::finish_part(&part, &dst).unwrap();
    assert!(fin.ends_with("x (2).flac"));
    assert_eq!(std::fs::read(&dst).unwrap(), std::fs::read(&src).unwrap());
}

#[test]
fn same_name_and_size_but_different_music_is_not_already_there() {
    let f = fixture();
    let dir = f.tmp.path().to_string_lossy().into_owned();
    let (a, b) = (join(&dir, "a.mp3"), join(&join(&dir, "d"), "a.mp3"));
    std::fs::write(&a, vec![1u8; 100_000]).unwrap();
    std::fs::create_dir_all(join(&dir, "d")).unwrap();
    std::fs::write(&b, vec![2u8; 100_000]).unwrap();
    let (st, fin, _) = copy::copy_one(&a, &b);
    assert_eq!(st, "copied");
    assert!(fin.ends_with("a (2).mp3"));
    assert_eq!(copy::copy_one(&a, &fin).0, "skipped"); // the real copy is recognised
}

#[test]
fn names_fit_any_stick() {
    let long = format!(
        "Artist - Album - 01 - {}End.flac",
        "Very Long Title ".repeat(14)
    );
    let plan = copy::plan_copies(
        &[Item {
            src: format!("/m/{long}"),
            rel: String::new(),
            n: Some(1),
        }],
        "/d",
        false,
        true,
    );
    let name = paths::basename(&plan[0].1);
    assert!(
        name.len() <= 240 && name.starts_with("001 - Artist") && name.ends_with(".flac"),
        "{name}"
    );
    let multi = format!("{}.mp3", "Ünïcödé ".repeat(40));
    let fitted = copy::safe(&multi);
    assert!(fitted.len() <= 240 && fitted.ends_with(".mp3")); // cut on a character boundary
    assert_eq!(copy::safe("Vol. 2..."), "Vol. 2_");
    assert_eq!(copy::safe("CON.mp3"), "_CON.mp3");
    assert_eq!(copy::safe("aux"), "_aux");
    assert_eq!(copy::safe("Auxiliary.mp3"), "Auxiliary.mp3");
}

#[test]
fn usb_without_ffmpeg_still_copies_what_decks_play() {
    let _f = fixture(); // holds the environment lock
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_string_lossy().into_owned();
    let (mp3, flac) = (join(&dir, "a.mp3"), join(&dir, "b.flac"));
    std::fs::write(&mp3, b"mp3").unwrap();
    std::fs::write(&flac, b"flac").unwrap();
    let path = std::env::var_os("PATH");
    std::env::set_var("PATH", "");
    let r1 = copy::convert_one(&mp3, &join(&dir, "usb/001 - a.mp3"), "aiff");
    let r2 = copy::convert_one(&flac, &join(&dir, "usb/002 - b.flac"), "aiff");
    if let Some(p) = path {
        std::env::set_var("PATH", p);
    }
    if crate::paths::WINDOWS {
        return; // winget's ffmpeg is found outside PATH on Windows
    }
    assert_eq!(r1.0, "copied");
    assert_eq!(r2.0, "error");
    assert!(r2.2.contains("ffmpeg"));
}

#[test]
fn hidden_files_and_symlink_loops_are_not_songs() {
    let f = fixture();
    let dir = paths::dirname(&join(
        &f.root,
        &LIBRARY[0].replace('/', &paths::SEP.to_string()),
    ))
    .to_string();
    std::fs::write(
        join(
            &dir,
            "._Depeche Mode - Violator - 06 - Enjoy the Silence.flac",
        ),
        b"mac junk",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("..", join(&dir, "loop")).unwrap();
    let lib = Library::new(&f.root);
    lib.scan(|_| {});
    assert_eq!(lib.len(), LIBRARY.len() - 1);
}

#[test]
fn go_back_finds_what_songcopy_kept() {
    let _f = fixture();
    let base = paths::app_base();
    let old = join(&join(&base, "songcopy"), "previous");
    std::fs::create_dir_all(&old).unwrap();
    let suffix = if paths::WINDOWS { ".exe" } else { "" };
    std::fs::write(
        join(&old, &format!("songcopy-2.0.2{suffix}")),
        b"old program",
    )
    .unwrap();
    let (v, p) = crate::update::previous().expect("the songcopy backup");
    assert_eq!(v, "2.0.2");
    assert!(p.contains("songcopy"));
}

#[test]
fn migration_is_done_once_and_never_overwrites() {
    let _g = TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_string_lossy().into_owned();
    std::env::set_var("LOCALAPPDATA", &base);
    let old = join(&base, "songcopy");
    let new = join(&base, "dubplate");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    std::fs::write(join(&old, "settings.json"), br#"{"server": "old"}"#).unwrap();
    std::fs::write(join(&old, "copied.json"), b"{}").unwrap();
    std::fs::write(join(&new, "settings.json"), br#"{"server": "new"}"#).unwrap(); // dubplate 2.1.0 already ran here
    paths::app_dir();
    assert_eq!(
        crate::discover::setting_str("server").as_deref(),
        Some("new")
    ); // not overwritten
    assert!(!paths::isfile(&join(&new, "copied.json"))); // dubplate already ran here: its folder stays as the user has it
    std::fs::remove_dir_all(&new).unwrap(); // the user resets dubplate...
    paths::app_dir();
    assert!(!paths::isfile(&join(&new, "settings.json"))); // ...and the old settings don't come back
}

#[test]
fn writers_never_corrupt_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x.json").to_string_lossy().into_owned();
    std::thread::scope(|s| {
        for t in 0..8 {
            let p = p.clone();
            s.spawn(move || {
                for i in 0..100 {
                    let body =
                        serde_json::json!({"t": t, "i": i, "pad": "x".repeat(5000)}).to_string();
                    paths::write_atomic(&p, body.as_bytes()).unwrap();
                }
            });
        }
    });
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert!(v.get("t").is_some());
    let stray = std::fs::read_dir(tmp.path()).unwrap().count();
    assert_eq!(stray, 1, "temporary files left behind");
}

#[test]
fn stale_temp_files_are_tidied_but_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().to_string_lossy().into_owned();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let make = |name: &str, aged: bool| {
        let p = join(&d, name);
        std::fs::write(&p, b"x").unwrap();
        if aged {
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        p
    };
    let stale = make(".dubplate-123-4.part", true);
    let fresh = make(".dubplate-123-5.part", false); // a copy still running elsewhere
    let song = make("Song.part", true); // not ours
    let lookalike = make(".dubplate-notes.part", true); // not ours either
    copy::clean_stale_parts(&d);
    assert!(!paths::exists(&stale));
    assert!(paths::exists(&fresh) && paths::exists(&song) && paths::exists(&lookalike));
}

// ---------------------------------------------------------------------------- second review round

#[test]
fn renaming_never_replaces_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().to_string_lossy().into_owned();
    let (a, b) = (join(&d, "a"), join(&d, "b"));
    std::fs::write(&a, b"new").unwrap();
    std::fs::write(&b, b"keep me").unwrap();
    let e = paths::rename_noreplace(&a, &b).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&b).unwrap(), b"keep me");
    let c = join(&d, "c");
    paths::rename_noreplace(&a, &c).unwrap();
    assert!(!paths::exists(&a) && std::fs::read(&c).unwrap() == b"new");
}

#[test]
fn names_cut_and_cleaned_correctly() {
    let bach = format!(
        "J.S. Bach: Mass in B minor, BWV 232 ({})",
        "Collegium Vocale Gent ".repeat(12)
    );
    let f = copy::safe(&bach);
    assert!(
        f.len() <= 240 && f.starts_with("J.S. Bach_ Mass in B minor"),
        "{f}"
    );
    assert_eq!(copy::safe("AC/DC"), "AC_DC");
    assert_eq!(copy::safe("../x"), ".._x");
    assert_eq!(copy::safe("Aux.Live.mp3"), "_Aux.Live.mp3");
    assert!(!copy::usb_folder("/stick", "../tmp/x").starts_with("/tmp"));
}

#[test]
fn band_names_with_dots_are_kept_and_junk_folders_skipped() {
    use crate::library::junk_folder;
    for keep in [
        ".38 Special",
        "...And You Will Know Us by the Trail of Dead",
        "A.F.I.",
        "Podcasts",
    ] {
        assert!(!junk_folder(keep), "{keep}");
    }
    for junk in [
        ".metube",
        ".Trash-1000",
        ".git",
        "@eaDir",
        "#recycle",
        "$RECYCLE.BIN",
    ] {
        assert!(junk_folder(junk), "{junk}");
    }
    let f = fixture();
    let p = join(
        &join(&f.root, ".38 Special"),
        ".38 Special - Hold On Loosely.mp3",
    );
    std::fs::create_dir_all(paths::dirname(&p)).unwrap();
    std::fs::write(&p, b"song").unwrap();
    let lib = Library::new(&f.root);
    lib.scan(|_| {});
    assert_eq!(lib.len(), LIBRARY.len()); // the .38 Special song is in
}

#[test]
fn an_earlier_copy_under_a_2_name_is_recognised() {
    let f = fixture();
    let dir = f.tmp.path().to_string_lossy().into_owned();
    let src = join(&dir, "song.mp3");
    std::fs::write(&src, vec![7u8; 1000]).unwrap();
    let dest = join(&dir, "out");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(join(&dest, "song.mp3"), vec![8u8; 1000]).unwrap(); // a different song, same name
    let first = copy::copy_one(&src, &join(&dest, "song.mp3"));
    assert_eq!(first.0, "copied");
    let again = copy::copy_one(&src, &join(&dest, "song.mp3"));
    assert_eq!((again.0, again.1.as_str()), ("skipped", first.1.as_str())); // no "song (3).mp3"
}

#[test]
fn usb_memory_is_per_playlist_folder_and_keeps_2_1_0_records() {
    let f = fixture();
    let src = join(&f.root, &LIBRARY[4].replace('/', &paths::SEP.to_string()));
    let stick = f.tmp.path().join("stick").to_string_lossy().into_owned();
    let (friday, saturday) = (
        copy::usb_folder(&stick, "Friday"),
        copy::usb_folder(&stick, "Saturday"),
    );
    let plan = copy::plan_copies(
        &[Item {
            src: src.clone(),
            rel: String::new(),
            n: Some(1),
        }],
        &friday,
        false,
        true,
    );
    copy::transfer(&plan, &friday, None, |_, _| {});
    let srcs = [src.clone()];
    assert!(Ledger::copied_usb(&stick, &friday, &srcs).contains_key(&src));
    assert!(!Ledger::copied_usb(&stick, &saturday, &srcs).contains_key(&src)); // Saturday still gets it
                                                                               // dubplate 2.1.0 filed USB copies under the whole stick: still recognised for their folder
    let plan = copy::plan_copies(
        &[Item {
            src: src.clone(),
            rel: String::new(),
            n: Some(2),
        }],
        &saturday,
        false,
        true,
    );
    copy::transfer(&plan, &stick, None, |_, _| {});
    assert!(Ledger::copied_usb(&stick, &saturday, &srcs).contains_key(&src));
}

#[test]
fn ffmpeg_complaints_are_readable() {
    let e = "[flac @ 0x5fe18707ecc0] invalid sync code\n[in#0 @ 0x5fe1] Error opening input: Invalid data found when processing input\n[aost#0:0/pcm_s16be @ 0x5fe1] Task finished with error code: -22 (Invalid argument)\n";
    let s = super::copy::tests_reason(e);
    assert_eq!(s, "invalid sync code");
}
