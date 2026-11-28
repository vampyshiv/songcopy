//! Traktor: reading .nml files (a whole collection, or a playlist exported from Traktor).
//!
//! Used two ways: a Traktor playlist as the song list, and the collection's beat grid, BPM,
//! key and cue points for songs that go on a rekordbox stick.

use crate::library::Request;
use crate::paths::{basename, normcase};
use serde_json::Value;
use std::collections::HashMap;

/// a cue point as Traktor stores it (CUE_V2)
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub name: String,
    /// 0 cue, 1 fade-in, 2 fade-out, 3 load, 4 grid, 5 loop
    pub kind: u8,
    pub start_ms: f64,
    pub len_ms: f64,
    /// hot cue 0-7 (Traktor's 1-8), or None
    pub hotcue: Option<u8>,
}

/// one song in a Traktor collection
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// where Traktor has the file, as a normal path ("C:/Users/me/Music/x.mp3", "/Users/me/x.mp3")
    pub path: String,
    /// file size in KB as Traktor recorded it (0 = unknown)
    pub size_kb: u64,
    pub bpm: Option<f64>,
    /// "Am", "F#", … (rekordbox's names)
    pub key: Option<String>,
    /// the beat grid's anchor (a downbeat), in ms
    pub grid_ms: Option<f64>,
    pub cues: Vec<Cue>,
}

/// a playlist (folders give it a path: "Gigs / Friday")
#[derive(Clone, Debug)]
pub struct Playlist {
    pub name: String,
    /// the collection keys of its songs, in order
    pub keys: Vec<String>,
}

#[derive(Default)]
pub struct Nml {
    /// by Traktor's own key (volume + path, as playlists refer to songs)
    pub entries: HashMap<String, Entry>,
    pub playlists: Vec<Playlist>,
}

const KEYS: [&str; 24] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B", "Cm", "Dbm", "Dm", "Ebm", "Em", "Fm", "F#m", "Gm",
    "Abm", "Am", "Bbm", "Bm",
];

/// Traktor's "C:/:Users/:me/:x.mp3" (or volume "Macintosh HD" + "/:Users/:me/:x.mp3") as a path
fn traktor_path(volume: &str, dir_and_file: &str) -> String {
    let rest = dir_and_file.replace("/:", "/");
    let b = volume.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        format!("{volume}{rest}") // Windows drive
    } else {
        rest // macOS: the volume name isn't part of the path (the startup disk is "/")
    }
}

/// a playlist's PRIMARYKEY ("C:/:Users/:me/:x.mp3", "Macintosh HD/:Users/:me/:x.mp3") as a path
fn key_path(key: &str) -> String {
    match key.find("/:") {
        Some(i) => traktor_path(&key[..i], &key[i..]),
        None => key.to_string(),
    }
}

fn attr<'a>(n: roxmltree::Node<'a, '_>, a: &str) -> &'a str {
    n.attribute(a).unwrap_or("")
}

fn num(n: roxmltree::Node<'_, '_>, a: &str) -> Option<f64> {
    n.attribute(a).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

/// read an .nml file
pub fn parse(text: &str) -> Result<Nml, String> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts)
        .map_err(|e| format!("this isn't a Traktor file dubplate can read ({e})"))?;
    let root = doc.root_element();
    if !root.has_tag_name("NML") {
        return Err("this isn't a Traktor .nml file".into());
    }
    let mut nml = Nml::default();
    if let Some(coll) = child(root, "COLLECTION") {
        for e in coll.children().filter(|c| c.has_tag_name("ENTRY")) {
            let Some(loc) = child(e, "LOCATION") else { continue };
            let (vol, dir, file) = (attr(loc, "VOLUME"), attr(loc, "DIR"), attr(loc, "FILE"));
            if file.is_empty() {
                continue;
            }
            let key = format!("{vol}{dir}{file}");
            let mut en = Entry {
                artist: attr(e, "ARTIST").trim().to_string(),
                title: attr(e, "TITLE").trim().to_string(),
                path: traktor_path(vol, &format!("{dir}{file}")),
                ..Default::default()
            };
            if let Some(a) = child(e, "ALBUM") {
                en.album = attr(a, "TITLE").trim().to_string();
            }
            if let Some(i) = child(e, "INFO") {
                en.size_kb = num(i, "FILESIZE").map_or(0, |v| v.max(0.0) as u64);
            }
            en.bpm = child(e, "TEMPO").and_then(|t| num(t, "BPM")).filter(|b| *b > 20.0 && *b < 400.0);
            en.key = child(e, "MUSICAL_KEY")
                .and_then(|k| num(k, "VALUE"))
                .and_then(|v| KEYS.get(v as usize).filter(|_| v >= 0.0 && v.fract() == 0.0))
                .map(|k| (*k).to_string());
            for c in e.children().filter(|c| c.has_tag_name("CUE_V2")) {
                let Some(start) = num(c, "START") else { continue };
                let kind = num(c, "TYPE").unwrap_or(0.0) as u8;
                let hot = num(c, "HOTCUE").filter(|h| (0.0..8.0).contains(h)).map(|h| h as u8);
                if kind == 4 && en.grid_ms.is_none() {
                    // the first grid marker anchors the beat grid
                    en.grid_ms = Some(start);
                }
                en.cues.push(Cue {
                    name: attr(c, "NAME").trim().to_string(),
                    kind,
                    start_ms: start,
                    len_ms: num(c, "LEN").unwrap_or(0.0).max(0.0),
                    hotcue: hot,
                });
            }
            nml.entries.insert(key, en);
        }
    }
    if let Some(pls) = child(root, "PLAYLISTS") {
        for top in pls.children().filter(|c| c.has_tag_name("NODE")) {
            walk(top, "", &mut nml.playlists);
        }
    }
    Ok(nml)
}

fn walk(node: roxmltree::Node<'_, '_>, parent: &str, out: &mut Vec<Playlist>) {
    let name = attr(node, "NAME");
    // the invisible top folder is "$ROOT"
    let here = if name == "$ROOT" || name.is_empty() {
        parent.to_string()
    } else if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent} / {name}")
    };
    match attr(node, "TYPE") {
        "PLAYLIST" => {
            if let Some(pl) = child(node, "PLAYLIST") {
                let keys = pl
                    .children()
                    .filter(|c| c.has_tag_name("ENTRY"))
                    .filter_map(|e| child(e, "PRIMARYKEY"))
                    .map(|k| attr(k, "KEY").to_string())
                    .filter(|k| !k.is_empty())
                    .collect();
                out.push(Playlist { name: here, keys });
            }
        }
        "FOLDER" => {
            if let Some(sub) = child(node, "SUBNODES") {
                for n in sub.children().filter(|c| c.has_tag_name("NODE")) {
                    walk(n, &here, out);
                }
            }
        }
        _ => {} // smart lists have no fixed songs
    }
}

/// the playlists worth offering: those with songs, Traktor's own empty ones left out
pub fn playlists_with_songs(nml: &Nml) -> Vec<(String, usize)> {
    nml.playlists.iter().filter(|p| !p.keys.is_empty()).map(|p| (p.name.clone(), p.keys.len())).collect()
}

/// one playlist as a song list. `which` names it; it may be left out when there's only one.
pub fn playlist_rows(nml: &Nml, which: Option<&str>) -> Result<Vec<Request>, String> {
    let pls: Vec<&Playlist> = nml.playlists.iter().filter(|p| !p.keys.is_empty()).collect();
    let pl = match which.filter(|w| !w.is_empty()) {
        Some(w) => pls.iter().find(|p| p.name == w).ok_or_else(|| format!("There's no playlist “{w}” in that file."))?,
        None => match pls.as_slice() {
            [] => return Err("That Traktor file has no playlists with songs in them.".into()),
            [one] => one,
            _ => return Err(CHOOSE.into()),
        },
    };
    Ok(pl
        .keys
        .iter()
        .enumerate()
        .map(|(n, k)| {
            let e = nml.entries.get(k);
            let path = e.map_or_else(|| key_path(k), |e| e.path.clone());
            let (artist, title, album) = e.map_or_else(Default::default, |e| (e.artist.clone(), e.title.clone(), e.album.clone()));
            let label = if !artist.is_empty() && !title.is_empty() {
                format!("{artist} - {title}")
            } else if !title.is_empty() {
                title.clone()
            } else {
                basename(&path.replace('\\', "/")).to_string()
            };
            Request { row: Value::from(n + 1), label: Some(label), artist, title, album, path, ..Default::default() }
        })
        .collect())
}

/// what playlist_rows says when the file has several playlists and none was chosen
pub const CHOOSE: &str = "choose a playlist";

/// a collection, looked up by file: the same file name and (where Traktor knows it) size
#[derive(Default)]
pub struct Collection {
    by_name: HashMap<String, Vec<Entry>>,
}

impl Collection {
    pub fn new(nml: Nml) -> Collection {
        let mut by_name: HashMap<String, Vec<Entry>> = HashMap::new();
        for e in nml.entries.into_values() {
            by_name.entry(normcase(basename(&e.path.replace('\\', "/")))).or_default().push(e);
        }
        Collection { by_name }
    }

    pub fn load(path: &str) -> Result<Collection, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("can't read the Traktor collection {path} ({e})"))?;
        Ok(Collection::new(parse(&crate::lists::decode(&bytes))?))
    }

    pub fn len(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    /// the entry for this file: same name, and the same size where Traktor recorded one.
    /// Paths aren't compared: Traktor may see the library as C:\… or \\server\… while dubplate
    /// sees /home/…
    pub fn find(&self, path: &str, size: u64) -> Option<&Entry> {
        let cands = self.by_name.get(&normcase(basename(&path.replace('\\', "/"))))?;
        let kb = size.div_ceil(1024);
        let sized: Vec<&Entry> =
            cands.iter().filter(|e| e.size_kb != 0 && (e.size_kb as i64 - kb as i64).abs() <= 1).collect();
        match sized.as_slice() {
            [one] => Some(one),
            [] if cands.len() == 1 && cands[0].size_kb == 0 => Some(&cands[0]),
            _ => None, // not there, or ambiguous: better no Traktor data than another song's
        }
    }
}

/// where Traktor keeps its collection on this computer, newest version first
/// (Documents/Native Instruments/Traktor 3.x/collection.nml)
pub fn find_collections() -> Vec<String> {
    let docs = crate::paths::join(&crate::paths::home(), "Documents");
    let ni = crate::paths::join(&docs, "Native Instruments");
    let mut found: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(&ni)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("Traktor"))
        .map(|d| d.path().join("collection.nml"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p.to_string_lossy().into_owned())))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Settings: "traktor_folder" is the Traktor folder holding collection.nml (or the .nml itself),
/// "off" to not use Traktor; not set = Traktor's usual place on this computer, if it's there
pub fn collection_file() -> Result<Option<String>, String> {
    match crate::discover::setting_str("traktor_folder").as_deref() {
        Some("off") => Ok(None),
        Some(f) => {
            let file = if f.to_lowercase().ends_with(".nml") { f.to_string() } else { crate::paths::join(f, "collection.nml") };
            if crate::paths::isfile(&file) {
                Ok(Some(file))
            } else {
                Err(format!("there's no collection.nml in {f} (Settings → Traktor)"))
            }
        }
        None => Ok(find_collections().into_iter().next()),
    }
}

/// the Traktor collection to take beat grids and cue points from, if there is one
pub fn collection() -> Result<Option<Collection>, String> {
    collection_file()?.map(|f| Collection::load(&f)).transpose()
}

// ---------------------------------------------------------------------------- to rekordbox

/// beats for the stick from Traktor's grid: one tempo, beat 1 on the anchor, from the first
/// beat at or after the start to the end of the song
pub fn beats(bpm: f64, anchor_ms: f64, duration_ms: f64) -> Vec<rbl_anlz::Beat> {
    let period = 60_000.0 / bpm;
    let tempo_x100 = (bpm * 100.0).round() as u16;
    let first = (-anchor_ms / period).ceil() as i64; // beats before the anchor, as a negative count
    let mut out = Vec::new();
    let mut k = first;
    loop {
        let t = anchor_ms + k as f64 * period;
        if t > duration_ms {
            break;
        }
        if t >= 0.0 {
            out.push(rbl_anlz::Beat { beat_number: (k.rem_euclid(4) + 1) as u16, tempo_x100, time_ms: t.round() as u32 });
        }
        k += 1;
    }
    out
}

/// Traktor's cue points as rekordbox ones: hot cues 1-8 become A-H, other cues and loops become
/// memory cues. The automatic grid marker and fade markers are left out.
pub fn cues(e: &Entry) -> Vec<rbl_anlz::cues::ExportCue> {
    let mut out = Vec::new();
    for c in &e.cues {
        let loop_end = (c.kind == 5 && c.len_ms > 0.0).then(|| (c.start_ms + c.len_ms).round().max(0.0) as u32);
        let kind = match c.hotcue {
            // rekordbox kinds: A-C are 1-3, D-P are 5-17 (4 is the active loop)
            Some(h) => if h < 3 { h + 1 } else { h + 2 },
            None if matches!(c.kind, 0 | 3 | 5) => 0,
            None => continue, // a plain grid or fade marker
        };
        let comment = if c.name.is_empty() || c.name == "n.n." || c.name == "AutoGrid" { String::new() } else { c.name.clone() };
        out.push(rbl_anlz::cues::ExportCue {
            kind,
            time_ms: c.start_ms.round().max(0.0) as u32,
            loop_time_ms: loop_end,
            comment,
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<NML VERSION="19"><HEAD COMPANY="www.native-instruments.com" PROGRAM="Traktor"></HEAD>
<COLLECTION ENTRIES="2">
<ENTRY MODIFIED_DATE="2024/1/1" TITLE="Nightdrive with You" ARTIST="Anoraak"><LOCATION DIR="/:Users/:dj/:Music/:Anoraak/:" FILE="01 - Nightdrive with You.flac" VOLUME="C:" VOLUMEID="abc"></LOCATION>
<ALBUM TRACK="1" TITLE="Nightdrive with You"></ALBUM><INFO BITRATE="1000000" FILESIZE="36326"></INFO>
<TEMPO BPM="117.999969" BPM_QUALITY="100.000000"></TEMPO><MUSICAL_KEY VALUE="21"></MUSICAL_KEY>
<CUE_V2 NAME="AutoGrid" DISPL_ORDER="0" TYPE="4" START="92.5" LEN="0.000000" REPEATS="-1" HOTCUE="0"></CUE_V2>
<CUE_V2 NAME="Drop" DISPL_ORDER="0" TYPE="0" START="61234.4" LEN="0.000000" REPEATS="-1" HOTCUE="3"></CUE_V2>
<CUE_V2 NAME="n.n." DISPL_ORDER="0" TYPE="5" START="30000" LEN="8135.6" REPEATS="-1" HOTCUE="-1"></CUE_V2>
<CUE_V2 NAME="n.n." DISPL_ORDER="0" TYPE="1" START="500" LEN="0" REPEATS="-1" HOTCUE="-1"></CUE_V2>
</ENTRY>
<ENTRY TITLE="Untitled"><LOCATION DIR="/:Users/:dj/:Music/:" FILE="track.mp3" VOLUME="Macintosh HD"></LOCATION></ENTRY>
</COLLECTION>
<PLAYLISTS><NODE TYPE="FOLDER" NAME="$ROOT"><SUBNODES COUNT="3">
<NODE TYPE="SMARTLIST" NAME="Recently added"><SMARTLIST UUID="x"></SMARTLIST></NODE>
<NODE TYPE="PLAYLIST" NAME="Preparation"><PLAYLIST ENTRIES="0" TYPE="LIST" UUID="a"></PLAYLIST></NODE>
<NODE TYPE="FOLDER" NAME="Gigs"><SUBNODES COUNT="1"><NODE TYPE="PLAYLIST" NAME="Friday"><PLAYLIST ENTRIES="3" TYPE="LIST" UUID="b">
<ENTRY><PRIMARYKEY TYPE="TRACK" KEY="Macintosh HD/:Users/:dj/:Music/:track.mp3"></PRIMARYKEY></ENTRY>
<ENTRY><PRIMARYKEY TYPE="TRACK" KEY="C:/:Users/:dj/:Music/:Anoraak/:01 - Nightdrive with You.flac"></PRIMARYKEY></ENTRY>
<ENTRY><PRIMARYKEY TYPE="TRACK" KEY="C:/:Users/:dj/:Music/:gone.mp3"></PRIMARYKEY></ENTRY>
</PLAYLIST></NODE></SUBNODES></NODE>
</SUBNODES></NODE></PLAYLISTS></NML>"#;

    #[test]
    fn reads_collection_and_playlists() {
        let n = parse(NML).unwrap();
        let e = &n.entries["C:/:Users/:dj/:Music/:Anoraak/:01 - Nightdrive with You.flac"];
        assert_eq!(e.path, "C:/Users/dj/Music/Anoraak/01 - Nightdrive with You.flac");
        assert_eq!((e.artist.as_str(), e.album.as_str(), e.size_kb), ("Anoraak", "Nightdrive with You", 36326));
        assert_eq!(e.key.as_deref(), Some("Am"));
        assert_eq!(e.grid_ms, Some(92.5));
        assert_eq!(n.entries["Macintosh HD/:Users/:dj/:Music/:track.mp3"].path, "/Users/dj/Music/track.mp3");
        // the empty "Preparation" and the smart list aren't offered
        assert_eq!(playlists_with_songs(&n), vec![("Gigs / Friday".to_string(), 3)]);
        let rows = playlist_rows(&n, None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].label.as_deref(), Some("Anoraak - Nightdrive with You"));
        assert_eq!(rows[1].path, "C:/Users/dj/Music/Anoraak/01 - Nightdrive with You.flac");
        // a song the collection doesn't describe still comes with its path
        assert_eq!((rows[2].path.as_str(), rows[2].label.as_deref()), ("C:/Users/dj/Music/gone.mp3", Some("gone.mp3")));
        assert!(playlist_rows(&n, Some("Nope")).is_err());
    }

    #[test]
    fn several_playlists_need_a_choice() {
        let two = NML.replace(
            r#"<NODE TYPE="PLAYLIST" NAME="Preparation"><PLAYLIST ENTRIES="0" TYPE="LIST" UUID="a"></PLAYLIST></NODE>"#,
            r#"<NODE TYPE="PLAYLIST" NAME="Preparation"><PLAYLIST ENTRIES="1" TYPE="LIST" UUID="a"><ENTRY><PRIMARYKEY TYPE="TRACK" KEY="C:/:x.mp3"></PRIMARYKEY></ENTRY></PLAYLIST></NODE>"#,
        );
        let n = parse(&two).unwrap();
        assert_eq!(playlist_rows(&n, None).unwrap_err(), CHOOSE);
        assert_eq!(playlist_rows(&n, Some("Preparation")).unwrap().len(), 1);
        assert!(parse("<html></html>").is_err());
    }

    #[test]
    fn finds_by_name_and_size() {
        let c = Collection::new(parse(NML).unwrap());
        // 36326 KB in Traktor; the file here is the same size (rounded up to whole KB)
        let e = c.find("/data/dl/Anoraak/01 - Nightdrive with You.flac", 36326 * 1024 - 100).unwrap();
        assert_eq!(e.title, "Nightdrive with You");
        assert!(c.find("/x/01 - Nightdrive with You.flac", 10 * 1024 * 1024).is_none(), "another size is another file");
        assert!(c.find("/x/track.mp3", 5).is_some(), "no size recorded, only one of that name");
    }

    #[test]
    fn grid_and_cues_for_rekordbox() {
        let n = parse(NML).unwrap();
        let e = &n.entries["C:/:Users/:dj/:Music/:Anoraak/:01 - Nightdrive with You.flac"];
        // 120 BPM = 500 ms a beat, anchor at 1200: beats at 200 (bar beat 3), 700, 1200 (1), …
        let b = beats(120.0, 1200.0, 3000.0);
        assert_eq!(b.iter().map(|x| (x.time_ms, x.beat_number)).collect::<Vec<_>>(), [(200, 3), (700, 4), (1200, 1), (1700, 2), (2200, 3), (2700, 4)]);
        assert_eq!(b[0].tempo_x100, 12000);
        let c = cues(e);
        // AutoGrid set as hot cue 1 -> A; "Drop" on hot cue 4 -> D (kind 5); the loop -> memory;
        // the fade-in marker is left out
        assert_eq!(c.iter().map(|x| (x.kind, x.time_ms, x.loop_time_ms)).collect::<Vec<_>>(), [(1, 93, None), (5, 61234, None), (0, 30000, Some(38136))]);
        assert_eq!((c[0].comment.as_str(), c[1].comment.as_str()), ("", "Drop"));
    }
}
