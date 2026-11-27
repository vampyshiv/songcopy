//! Rekordbox USB sticks: playlists in the deck's library browser, with beat grids, BPM, key and
//! waveforms - no rekordbox needed. Built on rbxport's engine (https://github.com/chrisle/rbxport,
//! GPL-2.0-or-later): rbl-analysis analyses each song, rbl-anlz writes the deck's analysis files,
//! rbl-export writes export.pdb / exportLibrary.db and converts audio older decks can't play.
//! `analyse_file` is adapted from rbxport's `src-tauri/src/analysis.rs` (v1.10.0).

use crate::paths::{self, join};
use rbl_export::{CompatibilityFormat, ExportOptions, SourcePlaylist, SourceTrack};
use serde::Serialize;
use std::path::Path;

/// bump when the analysis or how it's stored changes: old cached analysis is then redone
const CACHE_VERSION: &str = "rbl-1.10.0-a1";
/// analyse at most this much of a song (as rbxport does); longer files get the rest of their
/// waveform drawn but the tempo/key from the first half hour
const DECODE_CAP_SECS: f64 = 1800.0;

/// a song to put on the stick: the file, plus what dubplate already knows (used when the
/// file has no tags)
#[derive(Clone, Debug)]
pub struct Song {
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
}

#[derive(Default)]
struct Tags {
    title: String,
    artist: String,
    album: String,
    genre: String,
    label: String,
    key: String,
    comment: String,
    year: u16,
    track: u32,
    bitrate: u32,
    sample_rate: u32,
    bit_depth: u16,
}

fn read_tags(path: &str) -> Tags {
    use lofty::prelude::*;
    let Ok(f) = lofty::read_from_path(path) else { return Tags::default() };
    let p = f.properties();
    let mut t = Tags {
        bitrate: p.audio_bitrate().unwrap_or(0),
        sample_rate: p.sample_rate().unwrap_or(0),
        bit_depth: u16::from(p.bit_depth().unwrap_or(0)),
        ..Default::default()
    };
    if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
        let s = |v: Option<std::borrow::Cow<'_, str>>| v.map(|x| x.trim().to_string()).unwrap_or_default();
        t.title = s(tag.title());
        t.artist = s(tag.artist());
        t.album = s(tag.album());
        t.genre = s(tag.genre());
        t.comment = s(tag.comment());
        t.label = tag.get_string(ItemKey::Label).or_else(|| tag.get_string(ItemKey::Publisher)).unwrap_or("").trim().to_string();
        t.key = tag.get_string(ItemKey::InitialKey).unwrap_or("").trim().to_string();
        t.track = tag.track().unwrap_or(0);
        t.year = tag.date().map(|d| d.year).unwrap_or(0);
    }
    t
}

/// decode the file and analyse it the way rekordbox's Normal analysis does (one tempo fitted to
/// the whole song): beat grid, BPM, key, and every waveform the decks draw
fn analyse_file(path: &Path) -> Result<(rbl_analysis::Analysis, rbl_anlz::RekordboxWaves, f64), String> {
    let options = rbl_analysis::AnalysisPreset::Rekordbox.options();
    let mut stream = rbl_audio::MonoStream::open(path).map_err(|e| format!("can't read the audio ({e})"))?;
    let rate = stream.sample_rate();
    let mut creator = rbl_analysis::wave_creator::WaveCreator::new(rate);
    if let Some(frames) = stream.expected_frames() {
        creator.reserve(frames);
    }
    let cap = (DECODE_CAP_SECS * f64::from(rate)) as usize;
    let mut head: Vec<f32> = Vec::with_capacity(cap.min(1 << 24));
    let mut builder: Option<rbl_analysis::waveform::Builder> = None; // only for songs past the cap
    let mut past_head = false;
    while let Some((chunk, frames)) = stream.next_frames() {
        creator.push(frames);
        if past_head {
            if let Some(b) = builder.as_mut() {
                b.push(chunk);
            }
            continue;
        }
        head.extend_from_slice(chunk);
        if head.len() > cap {
            let mut b = rbl_analysis::waveform::Builder::new(rate);
            b.push(&head);
            builder = Some(b);
            head.truncate(cap);
            head.shrink_to_fit();
            past_head = true;
        }
    }
    if head.is_empty() {
        return Err("the file has no audio in it".into());
    }
    let mut analysis = rbl_analysis::analyse_with(&head, rate, options);
    let w = creator.finish();
    let waves = rbl_anlz::RekordboxWaves {
        pwav: w.pwav,
        pwv2: w.pwv2,
        pwvc: w.pwvc,
        pwv3: w.pwv3,
        pwv4: w.pwv4,
        pwv5: w.pwv5,
        pwv6: w.pwv6,
        pwv7: w.pwv7,
    };
    let mut duration = head.len() as f64 / f64::from(rate.max(1));
    if let Some(b) = builder {
        duration = b.samples() as f64 / f64::from(rate.max(1));
        analysis.peak = analysis.peak.max(b.peak());
        analysis.tempo.extend_to(duration);
        analysis.waveform = b.finish();
    }
    Ok((analysis, waves, duration))
}

/// where the analysis of this exact file (path, size, date) is kept
fn cache_key(path: &str) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let h = sha1_smol::Sha1::from(format!("{CACHE_VERSION}\n{path}\n{}\n{mtime}", m.len())).digest().to_string();
    Some(join(&join(&paths::app_dir(), "analysis"), &h[..2]).to_string() + &format!("{}{}", paths::SEP, &h[2..]))
}

fn load_cached(base: &str) -> Option<SourceTrack> {
    let mut t: SourceTrack = serde_json::from_slice(&std::fs::read(format!("{base}.json")).ok()?).ok()?;
    for ext in ["DAT", "EXT", "2EX"] {
        if let Ok(b) = std::fs::read(format!("{base}.{ext}")) {
            t.analysis.push((ext.to_string(), b));
        }
    }
    (!t.analysis.is_empty()).then_some(t)
}

fn save_cached(base: &str, t: &SourceTrack) {
    let mut meta = t.clone();
    meta.analysis.clear(); // the analysis files are stored as they are, next to it
    for (ext, bytes) in &t.analysis {
        let _ = paths::write_atomic(&format!("{base}.{ext}"), bytes);
    }
    if let Ok(j) = serde_json::to_vec(&meta) {
        let _ = paths::write_atomic(&format!("{base}.json"), &j);
    }
}

/// what goes on the stick for one song: its details and its analysis files. Analysed once,
/// then taken from dubplate's saved analysis until the file changes.
pub fn prepare(song: &Song) -> Result<SourceTrack, String> {
    let key = cache_key(&song.path);
    if let Some(t) = key.as_deref().and_then(load_cached) {
        return Ok(t);
    }
    let path = Path::new(&song.path);
    let tags = read_tags(&song.path);
    let (analysis, waves, duration) = analyse_file(path)?;
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|b| rbl_anlz::Beat { beat_number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms })
        .collect();
    let files = rbl_anlz::author_with_rekordbox_waves(
        "/pending", // rbl-export fills in the real path on the stick
        &beats,
        &waves,
        rbl_anlz::Existing { dat: None, ext: None, two_ex: None },
    );
    let pick = |tag: &str, fallback: &str| if tag.is_empty() { fallback.to_string() } else { tag.to_string() };
    let t = SourceTrack {
        id: 0,
        source_path: path.to_path_buf(),
        title: pick(&tags.title, &song.title),
        artist: pick(&tags.artist, &song.artist),
        album: pick(&tags.album, &song.album),
        genre: tags.genre,
        label: tags.label,
        key: analysis.key.as_ref().map_or(tags.key, |k| k.name.clone()),
        comment: tags.comment,
        duration_sec: duration.round().clamp(0.0, f64::from(u16::MAX)) as u16,
        bpm_x100: (analysis.tempo.bpm * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32,
        bitrate: tags.bitrate,
        sample_rate: tags.sample_rate,
        file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        year: tags.year,
        metadata: rbl_core::ExportMetadata {
            track_number: tags.track,
            bit_depth: tags.bit_depth,
            ..rbl_core::ExportMetadata::default()
        },
        cues: Some(Vec::new()),
        analysis: vec![("DAT".into(), files.dat), ("EXT".into(), files.ext), ("2EX".into(), files.two_ex)],
        ..SourceTrack::default()
    };
    if let Some(k) = key {
        save_cached(&k, &t);
    }
    Ok(t)
}

/// how far an export has got, for the page / terminal
#[derive(Serialize, Clone, Default, Debug)]
pub struct Progress {
    /// analysing | checking | copying | database | verifying | publishing | done
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct Report {
    pub playlist: String,
    pub tracks: usize,
    pub copied: usize,
    pub converted: usize,
    pub reused: usize,
    pub verified: bool,
    pub backup: Option<String>,
    pub errors: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// the playlist's id in the stick's record of where it came from: the same name each time, so
/// sending "Friday" again replaces Friday and sending "Saturday" adds a playlist next to it
fn playlist_id(name: &str) -> u64 {
    let d = sha1_smol::Sha1::from(name.to_lowercase()).digest().bytes();
    (u64::from_le_bytes(d[..8].try_into().unwrap()) & 0x7fff_ffff) | 1 // small and never 0
}

pub fn parse_convert(s: &str) -> Option<CompatibilityFormat> {
    match s {
        "aiff" => Some(CompatibilityFormat::Aiff),
        "mp3" => Some(CompatibilityFormat::Mp3),
        "wav" => Some(CompatibilityFormat::Wav),
        _ => None, // "none": the decks play FLAC (CDJ-2000NXS2, CDJ-3000 and newer)
    }
}

/// Write `songs` to the stick at `stick` as the playlist `name` (replacing a playlist of that
/// name; other playlists on the stick stay). Songs that can't be read are left out and listed.
pub fn export_set(
    stick: &str,
    name: &str,
    songs: &[Song],
    convert: Option<CompatibilityFormat>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Report, String> {
    if !paths::isdir(stick) {
        return Err(format!("{stick} isn't there (is the stick plugged in?)"));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the playlist a name.".into());
    }
    let backup = backup(stick)?;
    // analyse (or take the saved analysis), a few songs at a time
    let total = songs.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<Option<Result<SourceTrack, String>>>> = std::sync::Mutex::new(vec![None; total]);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
    progress(Progress { stage: "analysing".into(), done: 0, total, current: String::new() });
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= total {
                    break;
                }
                let r = prepare(&songs[i]);
                results.lock().unwrap()[i] = Some(r);
                let d = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                progress(Progress { stage: "analysing".into(), done: d, total, current: paths::basename(&songs[i].path).to_string() });
            });
        }
    });
    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for (song, r) in songs.iter().zip(results.into_inner().unwrap()) {
        match r {
            Some(Ok(t)) => tracks.push(t),
            Some(Err(e)) => errors.push((song.path.clone(), e)),
            None => errors.push((song.path.clone(), "not analysed".into())),
        }
    }
    if tracks.is_empty() {
        return Err("None of the songs could be read.".into());
    }
    let playlist = SourcePlaylist { id: playlist_id(name), name: name.to_string(), track_indices: (0..tracks.len()).collect(), ..Default::default() };
    // add to what the stick holds (other playlists stay), as rekordbox's "Export Playlist" does
    let options = ExportOptions { compatibility: convert, keep_unlisted: true, ..Default::default() };
    let n = tracks.len();
    let report = rbl_export::export_with_options(Path::new(stick), &tracks, &[playlist], &[], &options, &mut |p| {
        progress(Progress {
            stage: p.stage.to_string(),
            done: p.tracks_done,
            total: if p.tracks_total > 0 { p.tracks_total } else { n },
            current: String::new(),
        })
    })
    .map_err(|e| format!("couldn't write the stick: {e}"))?;
    progress(Progress { stage: "done".into(), done: n, total: n, current: String::new() });
    Ok(Report {
        playlist: name.to_string(),
        tracks: report.tracks,
        copied: report.tracks.saturating_sub(report.reused),
        converted: report.converted,
        reused: report.reused,
        verified: report.verified,
        backup,
        errors: errors
            .into_iter()
            .chain(report.skipped.iter().chain(&report.failed).map(|f| (String::new(), f.clone())))
            .collect(),
        warnings: report.warnings.iter().map(|w| format!("{w:?}")).collect(),
    })
}

// ---------------------------------------------------------------------------- backups

/// a name for the stick's backups: its path, made safe for a folder name
fn stick_id(stick: &str) -> String {
    let a = paths::abspath(stick);
    let h = sha1_smol::Sha1::from(paths::normcase(&a)).digest().to_string();
    let label: String = paths::basename(a.trim_end_matches(['/', '\\'])).chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').take(24).collect();
    format!("{}-{}", if label.is_empty() { "stick".into() } else { label }, &h[..8])
}

fn backups_dir(stick: &str) -> String {
    join(&join(&paths::app_dir(), "stick-backups"), &stick_id(stick))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            n += copy_tree(&p, &to.join(e.file_name()))?;
        } else {
            n += std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(n)
}

/// the folders on a stick that hold its library (not the music): rekordbox's databases, and
/// rbxport's record of what it put there
const LIBRARY_DIRS: [&str; 2] = ["rekordbox", "rbxport"];

/// Keep a copy of the stick's library (its databases, not the music), so an export can be
/// undone. Nothing to keep on a stick without one. The newest 10 per stick are kept.
pub fn backup(stick: &str) -> Result<Option<String>, String> {
    let root = rbl_export::export_root(Path::new(stick));
    if !root.join("rekordbox").is_dir() {
        return Ok(None);
    }
    let mut when = chrono::Local::now().format("%Y-%m-%d %H.%M.%S").to_string();
    let base = backups_dir(stick);
    let mut n = 2;
    while paths::exists(&join(&base, &when)) {
        when = format!("{} ({n})", &when[..19]);
        n += 1;
    }
    let dir = join(&base, &when);
    let fail = |e: std::io::Error| {
        let _ = std::fs::remove_dir_all(&dir);
        format!("couldn't back up the stick's library first ({e}) - nothing was changed")
    };
    for d in LIBRARY_DIRS {
        if root.join(d).is_dir() {
            copy_tree(&root.join(d), &Path::new(&dir).join(d)).map_err(fail)?;
        }
    }
    let mut all = list_backups(stick);
    while all.len() > 10 {
        if let Some(old) = all.pop() {
            let _ = std::fs::remove_dir_all(join(&base, &old));
        }
    }
    Ok(Some(when))
}

/// the stick's backups, newest first
pub fn list_backups(stick: &str) -> Vec<String> {
    let mut v: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(backups_dir(stick))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("rekordbox").is_dir())
                .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.into_iter().rev().map(|(_, n)| n).collect()
}

/// make `to` hold exactly what `from` holds (files only `to` has are removed)
fn mirror(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.is_dir() {
        for e in std::fs::read_dir(to)? {
            let e = e?;
            let p = e.path();
            if !from.join(e.file_name()).exists() {
                if p.is_dir() { std::fs::remove_dir_all(&p)? } else { std::fs::remove_file(&p)? }
            }
        }
    }
    copy_tree(from, to).map(|_| ())
}

/// put a backup of the stick's library back (what it holds now is backed up first). The music
/// on the stick is left alone; songs only a later export used simply stop being listed.
pub fn restore(stick: &str, which: &str) -> Result<(), String> {
    if which.is_empty() || which.contains(['/', '\\']) || which.contains("..") {
        return Err("no such backup".into());
    }
    let from = join(&backups_dir(stick), which);
    if !Path::new(&from).join("rekordbox").is_dir() {
        return Err(format!("no backup called {which} for this stick"));
    }
    backup(stick)?;
    let root = rbl_export::export_root(Path::new(stick));
    for d in LIBRARY_DIRS {
        let (f, t) = (Path::new(&from).join(d), root.join(d));
        let r = if f.is_dir() { mirror(&f, &t) } else if t.is_dir() { std::fs::remove_dir_all(&t) } else { Ok(()) };
        r.map_err(|e| format!("couldn't put the backup back ({e}); the newest backup holds what the stick had"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a click track: a short burst every beat at `bpm`, as a WAV file
    pub fn click_wav(path: &str, bpm: f64, seconds: u32) {
        let rate = 44100u32;
        let n = rate * seconds;
        let beat = (60.0 / bpm * f64::from(rate)) as u32;
        let mut data = Vec::with_capacity(n as usize * 2);
        for i in 0..n {
            let s: i16 = if i % beat < 400 { if (i / 20) % 2 == 0 { 20000 } else { -20000 } } else { 0 };
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        std::fs::write(path, w).unwrap();
    }

    #[test]
    fn a_rekordbox_stick_from_loose_files() {
        let _g = crate::paths::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LOCALAPPDATA", tmp.path().join("app"));
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let songs: Vec<Song> = [(120.0, "First"), (128.0, "Second")]
            .iter()
            .map(|(bpm, t)| {
                let p = src.join(format!("Artist - {t}.wav")).to_string_lossy().into_owned();
                click_wav(&p, *bpm, 20);
                Song { path: p, artist: "Artist".into(), title: t.to_string(), album: String::new() }
            })
            .collect();
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let st = stick.to_string_lossy().into_owned();
        let r = export_set(&st, "Friday", &songs, None, &|_| {}).unwrap();
        assert_eq!(r.tracks, 2, "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(r.backup.is_none()); // nothing was on the stick yet
        assert!(stick.join("PIONEER/rekordbox/export.pdb").is_file());
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.tracks == 2 && v.playlists == 1 && v.analysis_present == 2, "{v:?}");
        // the analysis found the tempo
        let t = prepare(&songs[1]).unwrap();
        assert!((t.bpm_x100 as i32 - 12800).abs() <= 100, "bpm {}", t.bpm_x100);
        // a second set goes next to the first; the stick is backed up first
        let r2 = export_set(&st, "Saturday", &songs[..1], None, &|_| {}).unwrap();
        assert!(r2.backup.is_some());
        let v = rbl_export::verify(&stick).unwrap();
        assert_eq!(v.playlists, 2, "{v:?}");
        assert_eq!(list_backups(&st).len(), 1);
        // sending Friday again (any case) replaces it rather than adding a third
        export_set(&st, "friday", &songs[1..], None, &|_| {}).unwrap();
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.playlists == 2, "{v:?}");
        assert_eq!(list_backups(&st).len(), 2);
        let all = list_backups(&st);
        let first = all.last().unwrap().clone(); // oldest: before Saturday
        // the backup can be put back
        restore(&st, &first).unwrap();
        assert_eq!(rbl_export::verify(&stick).unwrap().playlists, 1);
        assert!(restore(&st, "../x").is_err() && restore(&st, "nope").is_err());
        // and the stick takes more sets after a restore
        export_set(&st, "Sunday", &songs, None, &|_| {}).unwrap();
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.playlists == 2 && v.tracks == 2, "{v:?}");
    }
}
