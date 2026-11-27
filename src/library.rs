//! The music library: reading it, the search index, and matching a requested song to a file.

use crate::paths::{self, abspath, basename, dirname, expanduser, is_audio, join, splitext, SEP};
use crate::text::{
    clean_title, close_matches, first_artist, norm, ratio, round3, same_version, split_version,
    tokens,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, RwLock};

pub const SKIP_DIRS: &[&str] = &["Podcasts"]; // top-level folders that aren't songs

/// folders that hold no music: apps' and systems' hidden folders (".metube", ".Trash", ".git":
/// a dot then a letter) and NAS/Windows housekeeping. Band names that start with a dot
/// (".38 Special", "...And You Will Know Us by the Trail of Dead") are kept.
pub fn junk_folder(name: &str) -> bool {
    let hidden_app =
        name.starts_with('.') && name[1..].starts_with(|c: char| c.is_ascii_alphabetic());
    hidden_app
        || matches!(
            name,
            "@eaDir"
                | "#recycle"
                | "#snapshot"
                | "$RECYCLE.BIN"
                | "System Volume Information"
                | "lost+found"
        )
}

/// the name shown in front of songs from an "also look in" folder: the folder's name ("Drive E"
/// for a whole drive), made unique when two folders have the same name
fn unique_label(root: &str, taken: &[String]) -> String {
    let trimmed = root.trim_end_matches(['/', '\\']);
    let mut l = basename(trimmed).to_string();
    if l.is_empty() || l.ends_with(':') {
        l = match trimmed.chars().next() {
            Some(d) if trimmed.len() <= 2 && d.is_ascii_alphabetic() => {
                format!("Drive {}", d.to_ascii_uppercase())
            }
            _ => "other folder".to_string(),
        };
    }
    let mut out = l.clone();
    let mut k = 2;
    while taken.contains(&out) {
        out = format!("{l} ({k})");
        k += 1;
    }
    out
}

/// clears the "scanning" flag however a scan ends, even if it fails, so the library can
/// always be read again
struct ScanFlag<'a>(&'a AtomicBool);
impl Drop for ScanFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

static FNAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<artist>.+?) - (?P<album>.+) - (?P<track>\d{1,3}) - (?P<title>.+)$").unwrap()
});
static LEADING_TRACKNO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}[\s.\-_]+").unwrap());
static YEAR_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d{4}\)$").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{4})\)").unwrap());
static NOT_ORIGINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\d{4}-\d\d-\d\d|\b(live|tour\w*|concert|festival|arena|stadium|unplugged|sessions?|best of|greatest|hits|collection|anthology|essential|",
        r"singles|rarities|remix(es)?|mixes|b.sides|demos?|karaoke|instrumentals?|compilation|vol\.?\s*\d)\b|",
        r"\b(cd|disc)\s*\d"
    ))
    .unwrap()
});
static AUDIO_NAME: LazyLock<Regex> = LazyLock::new(|| {
    let exts: Vec<&str> = paths::AUDIO_EXT.iter().map(|e| &e[1..]).collect();
    Regex::new(&format!(r"(?i)\.({})$", exts.join("|"))).unwrap()
});
static FREE_TEXT_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[-–—]\s+|\t").unwrap());

/// One song file in the library.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Track {
    pub path: String,
    pub rel: String,
    pub name: String,
    pub ext: String,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub size: u64,
    #[serde(default)]
    pub n_dirartist: String,
    #[serde(skip)]
    pub n_artist: String,
    #[serde(skip)]
    pub n_title: String,
    #[serde(skip)]
    pub n_album: String,
    #[serde(skip)]
    pub n_base: String,
    #[serde(skip)]
    pub vtags: String,
}

impl Track {
    pub fn fill_norms(&mut self) {
        self.n_artist = norm(&self.artist);
        self.n_title = norm(&self.title);
        self.n_album = norm(&self.album);
        let (b, v) = split_version(&self.title);
        self.n_base = norm(&b);
        self.vtags = v;
    }
}

/// What the page / CLI sees of a file.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct PubFile {
    pub path: String,
    pub rel: String,
    pub name: String,
    pub ext: String,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub size: u64,
    pub score: f64,
}

fn publish(f: &Track, score: f64) -> PubFile {
    PubFile {
        path: f.path.clone(),
        rel: f.rel.clone(),
        name: f.name.clone(),
        ext: f.ext.clone(),
        artist: f.artist.clone(),
        album: f.album.clone(),
        title: f.title.clone(),
        size: f.size,
        score: round3(score),
    }
}

/// A song asked for (a row of the list).
#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub struct Request {
    #[serde(default)]
    pub row: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub raw: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MatchResult {
    pub row: Value,
    pub label: String,
    /// found | guess | version | missing
    pub status: String,
    pub file: Option<PubFile>,
    pub alternatives: Vec<PubFile>,
    pub suggestions: Vec<PubFile>,
    pub note: String,
    /// the artist / title / album it looked for (after reading the list row), e.g. for Soulseek
    pub ask: Ask,
    #[serde(skip)]
    pub n: usize,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct Ask {
    pub artist: String,
    pub title: String,
    pub album: String,
}

#[derive(Default)]
struct Data {
    files: Vec<Track>,
    by_name: HashMap<String, Vec<u32>>,
    index: HashMap<String, Vec<u32>>,
    scanned_at: Option<chrono::NaiveDateTime>,
}

pub struct Library {
    /// the main music folder (changeable in Settings)
    root: RwLock<String>,
    /// "also look in" folders (e.g. where a downloader saves songs), chosen by the user
    extra: RwLock<Vec<String>>,
    data: RwLock<Data>,
    pub scanning: AtomicBool,
    /// asked to scan again while a scan was running
    again: AtomicBool,
    pub progress: AtomicUsize,
    ready: (Mutex<bool>, Condvar),
}

type Scored<'a> = Vec<(f64, &'a Track, u32)>;

/// The current library contents, for matching (held for the duration of one request).
pub struct View<'a> {
    d: std::sync::RwLockReadGuard<'a, Data>,
    root: String,
}

impl Library {
    pub fn new(root: &str) -> Library {
        Library {
            root: RwLock::new(abspath(&expanduser(root))),
            extra: RwLock::new(Vec::new()),
            data: RwLock::new(Data::default()),
            scanning: AtomicBool::new(false),
            again: AtomicBool::new(false),
            progress: AtomicUsize::new(0),
            ready: (Mutex::new(false), Condvar::new()),
        }
    }

    /// the main music folder
    pub fn root(&self) -> String {
        self.root.read().unwrap().clone()
    }

    /// change the main music folder (takes effect with the next scan)
    pub fn set_root(&self, root: &str) {
        *self.root.write().unwrap() = abspath(&expanduser(root));
    }

    /// set the "also look in" folders (takes effect with the next scan)
    pub fn set_extra(&self, folders: &[String]) {
        let root = self.root();
        let mut v: Vec<String> = Vec::new();
        for f in folders {
            let f = abspath(&expanduser(f));
            if f != root && !v.contains(&f) && !f.is_empty() {
                v.push(f);
            }
        }
        *self.extra.write().unwrap() = v;
    }

    /// every folder the library is read from: the main one first
    pub fn roots(&self) -> Vec<String> {
        std::iter::once(self.root())
            .chain(self.extra.read().unwrap().iter().cloned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.data.read().unwrap().files.len()
    }

    pub fn scanned_at(&self) -> Option<chrono::NaiveDateTime> {
        self.data.read().unwrap().scanned_at
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.0.lock().unwrap()
    }

    pub fn wait_ready(&self) {
        let mut r = self.ready.0.lock().unwrap();
        while !*r {
            r = self.ready.1.wait(r).unwrap();
        }
    }

    pub fn view(&self) -> View<'_> {
        View {
            d: self.data.read().unwrap(),
            root: self.root(),
        }
    }

    /// read every audio file in every library folder (depth first, sorted). Songs from the
    /// "also look in" folders get that folder's name in front of their path, so it's clear where
    /// they are. A folder that's inside another one is only read once.
    fn walk(&self, roots: &[String], mut each: impl FnMut(String, String, &str, u64)) {
        let mut visited: HashSet<std::path::PathBuf> = HashSet::new();
        let mut labels: Vec<String> = Vec::new();
        for (i, root) in roots.iter().enumerate() {
            let label = if i == 0 {
                String::new()
            } else {
                unique_label(root, &labels)
            };
            labels.push(label.clone());
            self.walk_root(root, &label, &mut visited, &mut each);
        }
    }

    fn walk_root(
        &self,
        root: &str,
        label: &str,
        visited: &mut HashSet<std::path::PathBuf>,
        each: &mut impl FnMut(String, String, &str, u64),
    ) {
        let mut stack: Vec<(String, String)> = vec![(root.to_string(), String::new())];
        // folders already read (by their real location): a symlink pointing back up the tree
        // would otherwise list the same songs again and again
        let real_root = std::fs::canonicalize(root).ok();
        while let Some((dir, rel)) = stack.pop() {
            if let Ok(real) = std::fs::canonicalize(&dir) {
                if !visited.insert(real) {
                    continue;
                }
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
            let mut subdirs = Vec::new();
            for e in entries {
                let Some(name) = e.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if name.starts_with("._") {
                    continue; // macOS leftovers ("._song.flac"), not music
                }
                let Ok(meta) = std::fs::metadata(e.path()) else {
                    continue;
                }; // follows symlinks
                let r = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}{SEP}{name}")
                };
                if meta.is_dir() {
                    if (rel.is_empty() && SKIP_DIRS.contains(&name.as_str())) || junk_folder(&name)
                    {
                        continue;
                    }
                    // a shortcut (symlink) to a folder that's inside the library anyway: the real
                    // folder is read under its own name, so its songs keep their real artist/album
                    let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                    if is_link {
                        if let (Some(root), Ok(target)) =
                            (&real_root, std::fs::canonicalize(e.path()))
                        {
                            if target.starts_with(root) {
                                continue;
                            }
                        }
                    }
                    subdirs.push((join(&dir, &name), r));
                } else if is_audio(&name) {
                    each(join(root, &r), r, label, meta.len());
                }
            }
            stack.extend(subdirs.into_iter().rev());
        }
    }

    /// Read the library (all its folders). If a scan is already running, it's asked to go
    /// round once more instead (so a folder added in Settings meanwhile isn't missed).
    pub fn scan(&self, mut on_progress: impl FnMut(usize)) -> usize {
        if self
            .scanning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.again.store(true, Ordering::SeqCst);
            return self.len();
        }
        self.scan_running(&mut on_progress)
    }

    /// start a scan in the background; the "scanning" flag is set before this returns, so a
    /// page asking straight after sees that the library is being read
    pub fn start_scan(self: &std::sync::Arc<Self>) {
        if self
            .scanning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.again.store(true, Ordering::SeqCst);
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            me.scan_running(&mut |_| {});
        });
    }

    fn scan_running(&self, on_progress: &mut dyn FnMut(usize)) -> usize {
        let _flag = ScanFlag(&self.scanning);
        loop {
            self.again.store(false, Ordering::SeqCst);
            self.progress.store(0, Ordering::SeqCst);
            let roots = self.roots();
            let mut files: Vec<Track> = Vec::new();
            self.walk(&roots, |full, rel, label, size| {
                // the song is read from its path inside its own folder ("Album/01 - Title.flac"),
                // and shown with that folder's name in front ("downloads/Album/01 - Title.flac")
                let mut t = parse_track(full, rel, size);
                if !label.is_empty() {
                    t.rel = format!("{label}{SEP}{}", t.rel);
                }
                files.push(t);
                self.progress.store(files.len(), Ordering::Relaxed);
                on_progress(files.len());
            });
            self.install(files, None);
            self.save_cache(&roots.join("\n"));
            // folders changed in Settings while reading, or another scan was asked for: go again
            if self.roots() == roots && !self.again.load(Ordering::SeqCst) {
                break;
            }
        }
        self.len()
    }

    fn install(&self, mut files: Vec<Track>, when: Option<chrono::NaiveDateTime>) {
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::new();
        let mut index: HashMap<String, Vec<u32>> = HashMap::new();
        for (i, f) in files.iter_mut().enumerate() {
            f.fill_norms();
            let i = i as u32;
            let (stem, _) = splitext(&f.name);
            by_name.entry(f.name.to_lowercase()).or_default().push(i);
            by_name.entry(stem.to_lowercase()).or_default().push(i);
            let without_ext = match f.rel.rfind('.') {
                Some(p) => &f.rel[..p],
                None => &f.rel[..],
            };
            let toks: HashSet<String> =
                tokens(&without_ext.replace(SEP, " ")).into_iter().collect();
            for t in toks {
                index.entry(t).or_default().push(i);
            }
        }
        {
            let mut d = self.data.write().unwrap();
            *d = Data {
                files,
                by_name,
                index,
                scanned_at: Some(when.unwrap_or_else(|| chrono::Local::now().naive_local())),
            };
        }
        *self.ready.0.lock().unwrap() = true;
        self.ready.1.notify_all();
    }

    // A saved copy of the song list, so the page opens instantly next time (matters on Windows,
    // where reading 25,000 files over the network takes a while). Always refreshed in the background.
    fn roots_key(&self) -> String {
        self.roots().join("\n")
    }

    fn cache_file(&self) -> String {
        let h = sha1_smol::Sha1::from(self.roots_key().as_bytes())
            .digest()
            .to_string();
        join(&paths::app_dir(), &format!("{}.json", &h[..12]))
    }

    fn save_cache(&self, key: &str) {
        let d = self.data.read().unwrap();
        let body = serde_json::json!({
            "root": key,
            "when": d.scanned_at.map(|t| t.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
            "files": d.files,
        });
        let h = sha1_smol::Sha1::from(key.as_bytes()).digest().to_string();
        let _ = paths::write_atomic(
            &join(&paths::app_dir(), &format!("{}.json", &h[..12])),
            body.to_string().as_bytes(),
        );
    }

    pub fn load_cache(&self) -> bool {
        #[derive(Deserialize)]
        struct Cache {
            root: String,
            when: Option<String>,
            files: Vec<Track>,
        }
        let Ok(s) = std::fs::read_to_string(self.cache_file()) else {
            return false;
        };
        let Ok(c) = serde_json::from_str::<Cache>(&s) else {
            return false;
        };
        if c.root != self.roots_key() || c.files.is_empty() {
            return false;
        }
        let when = c
            .when
            .and_then(|w| chrono::NaiveDateTime::parse_from_str(&w, "%Y-%m-%dT%H:%M:%S%.f").ok());
        self.install(c.files, when);
        true
    }
}

/// what a file's path says about the song: "Artist - Album - 07 - Title.flac" in
/// "Artist/Album (Year)/", or loose "Artist - Title.mp3"
pub fn parse_track(full: String, rel: String, size: u64) -> Track {
    let name = basename(&rel).to_string();
    let (stem, ext) = splitext(&name);
    let parts: Vec<&str> = rel.split(SEP).collect();
    let np = parts.len();
    let (artist, album, title) = if let Some(m) = FNAME_RE.captures(stem) {
        (
            m["artist"].to_string(),
            m["album"].to_string(),
            m["title"].to_string(),
        )
    } else {
        // "Artist - Title" loose file, or something else
        let bits: Vec<&str> = stem.split(" - ").collect();
        let title = LEADING_TRACKNO
            .replace(bits[bits.len() - 1], "")
            .to_string();
        let artist = if bits.len() > 1 {
            bits[0].to_string()
        } else if np >= 3 {
            parts[np - 3].to_string()
        } else {
            String::new()
        };
        let album = if np >= 3 {
            parts[np - 2].to_string()
        } else {
            String::new()
        };
        (artist, YEAR_SUFFIX.replace(&album, "").to_string(), title)
    };
    Track {
        path: full,
        ext: ext.to_lowercase().trim_start_matches('.').to_string(),
        n_dirartist: if np >= 3 {
            norm(parts[np - 3])
        } else {
            String::new()
        },
        name,
        rel,
        artist,
        album,
        title,
        size,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------- matching

fn album_rank(f: &Track) -> (u8, u32) {
    let parent = dirname(&f.rel);
    let year = YEAR
        .captures(parent)
        .and_then(|m| m[1].parse().ok())
        .unwrap_or(9999);
    (u8::from(NOT_ORIGINAL.is_match(parent)), year)
}

pub fn same_song(title: &str, f: &Track) -> bool {
    let rb = norm(&split_version(title).0);
    !rb.is_empty() && (rb == f.n_base || (rb.chars().count() >= 6 && ratio(&rb, &f.n_base) >= 0.92))
}

/// 0..1, how well file f matches what was asked for
pub fn score(f: &Track, artist: &str, title: &str, album: &str) -> f64 {
    let mut s = 0.65 * title_score(f, title) + 0.35 * artist_score(f, artist);
    if !album.is_empty() && norm(album) == f.n_album {
        s += 0.05;
    }
    s.min(1.0)
}

/// 0..1: how well the file's title matches (versions included)
pub fn title_score(f: &Track, title: &str) -> f64 {
    let nt = norm(title);
    if nt.is_empty() {
        0.5
    } else if nt == f.n_title {
        1.0
    } else {
        let mut s = ratio(&nt, &f.n_title);
        if same_song(title, f) {
            let (base, rt) = split_version(title);
            if rt == f.vtags {
                // same recording: "Avichi" = "Avichi (Original Mix)", "- Remastered 2011"
                s = s.max(if norm(&base) == f.n_base { 0.97 } else { 0.93 });
            } else if same_version(&rt, &f.vtags) {
                s = s.max(0.88); // typo in the version name
            } else {
                // same song, different version: the plain original first, then the most similar other one
                s = if f.vtags.is_empty() {
                    0.86
                } else {
                    0.8 + 0.05 * s
                };
            }
        }
        s
    }
}

/// 0..1: how well the file's artist matches (0.8 when none was asked for)
pub fn artist_score(f: &Track, artist: &str) -> f64 {
    let na = norm(artist);
    if na.is_empty() {
        return 0.8;
    }
    let fa = &f.n_artist;
    if na == *fa || na == f.n_dirartist {
        1.0
    } else if fa.contains(&na) || na.contains(fa.as_str()) || f.n_dirartist.contains(&na) {
        0.9
    } else {
        ratio(&na, fa)
    }
}

const NOTE_VERSION: &str =
    "you don't have this version - only the one shown here (or pick another from the list). Not copied unless you tick it.";

impl View<'_> {
    /// files whose path contains ALL of these words (sorted)
    fn candidates(&self, toks: &[String]) -> Vec<u32> {
        let mut seen = HashSet::new();
        let toks: Vec<&String> = toks
            .iter()
            .filter(|t| !t.is_empty() && seen.insert(t.as_str()))
            .collect();
        if toks.is_empty() {
            return vec![];
        }
        let mut posting: Vec<&Vec<u32>> = Vec::new();
        for t in toks {
            match self.d.index.get(t.as_str()) {
                Some(p) => posting.push(p),
                None => return vec![],
            }
        }
        posting.sort_by_key(|p| p.len());
        let mut out: Vec<u32> = posting[0].clone();
        for p in &posting[1..] {
            let s: HashSet<u32> = p.iter().copied().collect();
            out.retain(|i| s.contains(i));
            if out.is_empty() {
                break;
            }
        }
        out.sort_unstable();
        out
    }

    fn rank(
        &self,
        idxs: &[u32],
        artist: &str,
        title: &str,
        album: &str,
        prefer: &str,
    ) -> Scored<'_> {
        let pref: &[&str] = match prefer {
            "flac" => &["flac", "m4a", "opus", "mp3"],
            "mp3" => &["mp3", "m4a", "opus", "flac"],
            _ => &[],
        };
        let fmt_rank = |f: &Track| pref.iter().position(|e| *e == f.ext).unwrap_or(pref.len());
        let mut scored: Scored = idxs
            .iter()
            .map(|&i| {
                let f = &self.d.files[i as usize];
                (score(f, artist, title, album), f, i)
            })
            .collect();
        // higher score first; among equal scores: chosen format, then the original studio album
        // (not live / best-of / remix sets), earliest year, shortest path. "mp3" usually means
        // "my device can't play FLAC", so format wins there; for "flac" the original album wins.
        let mp3 = prefer == "mp3";
        scored.sort_by_cached_key(|(s, f, i)| {
            let (orig, year) = album_rank(f);
            let neg = -(round3(*s) * 1000.0).round() as i64;
            let fr = fmt_rank(f);
            if mp3 {
                (neg, fr as u32, orig as u32, year, f.rel.chars().count(), *i)
            } else {
                (neg, orig as u32, fr as u32, year, f.rel.chars().count(), *i)
            }
        });
        scored
    }

    fn finish(res: &mut MatchResult, scored: &Scored, status: &str, note: &str) {
        let mut seen = HashSet::new();
        let mut alts: Vec<PubFile> = Vec::new();
        for (sc, f, _) in scored {
            if seen.insert(f.path.as_str()) {
                alts.push(publish(f, *sc));
            }
        }
        res.file = Some(alts.remove(0));
        alts.truncate(24);
        res.alternatives = alts;
        res.status = status.to_string();
        res.note = note.to_string();
    }

    pub fn match_one(&self, req: &Request, prefer: &str) -> MatchResult {
        let mut artist = req.artist.trim().to_string();
        let mut title = req.title.trim().to_string();
        let mut album = req.album.trim().to_string();
        let path = req.path.trim().to_string();
        let mut raw = req.raw.trim().to_string();
        let label = req
            .label
            .clone()
            .filter(|l| !l.is_empty())
            .or_else(|| Some(raw.clone()).filter(|r| !r.is_empty()))
            .or_else(|| {
                let j: Vec<&str> = [artist.as_str(), title.as_str()]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect();
                Some(j.join(" - ")).filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| path.clone());
        let mut res = MatchResult {
            row: req.row.clone(),
            label,
            status: "missing".into(),
            file: None,
            alternatives: vec![],
            suggestions: vec![],
            note: String::new(),
            ask: Ask::default(),
            n: 0,
        };

        // 1. a real path or a filename
        let p = if !path.is_empty() {
            path.clone()
        } else if AUDIO_NAME.is_match(&raw) {
            raw.clone()
        } else {
            String::new()
        };
        if !p.is_empty() {
            let p2 = expanduser(&p.replace('\\', "/"));
            for cand in [p2.clone(), join(&self.root, p2.trim_start_matches('/'))] {
                if paths::isfile(&cand) && is_audio(&cand) {
                    let ap = abspath(&cand);
                    let f = self
                        .d
                        .files
                        .iter()
                        .find(|x| x.path == ap)
                        .cloned()
                        .unwrap_or_else(|| file_info(&ap, &self.root));
                    let scored = vec![(1.0, &f, 0)];
                    Self::finish(&mut res, &scored, "found", "");
                    return res;
                }
            }
            let base = basename(&p2).to_lowercase();
            let hits = self
                .d
                .by_name
                .get(&base)
                .or_else(|| self.d.by_name.get(splitext(&base).0));
            if let Some(hits) = hits {
                let scored: Scored = hits
                    .iter()
                    .map(|&i| (1.0, &self.d.files[i as usize], i))
                    .collect();
                let note = if !path.is_empty() && p2.contains('/') {
                    "same filename found in another folder"
                } else {
                    ""
                };
                Self::finish(&mut res, &scored, "found", note);
                return res;
            }
            // a playlist path from another computer: try to read artist/title out of the filename
            let stem = splitext(basename(&p2)).0.to_string();
            if let Some(m) = FNAME_RE.captures(&stem) {
                if artist.is_empty() {
                    artist = m["artist"].to_string();
                }
                if album.is_empty() {
                    album = m["album"].to_string();
                }
                if title.is_empty() {
                    title = m["title"].to_string();
                }
            } else if title.is_empty() {
                raw = stem;
            }
        }

        // 2. free text "Artist - Title" (or just words)
        if title.is_empty() && !raw.is_empty() {
            let bits: Vec<&str> = FREE_TEXT_SPLIT
                .split(&raw)
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .collect();
            if bits.len() >= 2 {
                artist = bits[0].to_string();
                title = bits[bits.len() - 1].to_string();
            } else {
                title = raw.clone();
                artist = String::new();
            }
        }

        res.ask = Ask {
            artist: artist.clone(),
            title: title.clone(),
            album: album.clone(),
        };
        // (words to search for) -> always scored against what was actually asked for
        let base = Some(split_version(&title).0)
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| title.clone());
        let fa = first_artist(&artist);
        let ct = clean_title(&title);
        let mut attempts: Vec<(String, String, String, String)> = Vec::new();
        for a in [
            (artist.clone(), title.clone(), artist.clone(), title.clone()),
            (artist.clone(), ct.clone(), artist.clone(), title.clone()),
            (artist.clone(), base.clone(), artist.clone(), title.clone()), // so other versions are found too
            (fa.clone(), ct.clone(), artist.clone(), title.clone()),
            (fa.clone(), base.clone(), artist.clone(), title.clone()),
        ] {
            if !attempts.contains(&a) {
                attempts.push(a);
            }
        }
        if artist.is_empty() && !raw.is_empty() {
            attempts.push((String::new(), raw.clone(), String::new(), raw.clone()));
        }
        if !artist.is_empty() && !title.is_empty() {
            attempts.push((title.clone(), artist.clone(), title.clone(), artist.clone()));
            // written backwards
        }

        let mut best: Option<Scored> = None;
        for (a, t, sa, st) in &attempts {
            let mut toks = tokens(a);
            toks.extend(tokens(t));
            let idxs = self.candidates(&toks);
            if idxs.is_empty() {
                continue;
            }
            let scored = self.rank(&idxs, sa, st, &album, prefer);
            let top = scored[0].0;
            if best.as_ref().is_none_or(|b| top > b[0].0) {
                best = Some(scored);
            }
            if top >= 0.97 {
                break;
            }
        }

        if let Some(b) = &best {
            let top_score = b[0].0;
            let top = b[0].1;
            if top_score >= 0.6
                && !title.is_empty()
                && same_song(&title, top)
                && !same_version(&split_version(&title).1, &top.vtags)
                && norm(&title) != top.n_title
            {
                Self::finish(&mut res, b, "version", NOTE_VERSION);
                return res;
            }
            if top_score >= 0.94 {
                Self::finish(&mut res, b, "found", "");
                return res;
            }
            if top_score >= 0.6 {
                Self::finish(
                    &mut res,
                    b,
                    "guess",
                    "not an exact match - check it's the right song",
                );
                return res;
            }
        }

        // 3. nothing contains all the words: look for near misses (typos, missing words)
        let what = if title.is_empty() {
            raw.clone()
        } else {
            title.clone()
        };
        let sugg = self.fuzzy(&artist, &what, &album, prefer);
        if best.is_none() && !sugg.is_empty() && sugg[0].0 >= 0.82 {
            Self::finish(
                &mut res,
                &sugg,
                "guess",
                "spelled differently in your library - check it's the right song",
            );
            return res;
        }
        let mut pool: Scored = best.unwrap_or_default();
        pool.extend(sugg);
        pool.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap()); // stable, like Python
        let na = norm(&artist);
        let rb = norm(&split_version(&what).0);
        // don't suggest a Rammstein song for a Queen song just because both say "mix"
        let related = |f: &Track| {
            if na.is_empty() {
                return true;
            }
            if f.n_artist.contains(&na)
                || na.contains(f.n_artist.as_str())
                || f.n_dirartist.contains(&na)
                || ratio(&na, &f.n_artist) >= 0.7
            {
                return true;
            }
            !rb.is_empty() && ratio(&rb, &f.n_base) >= 0.85
        };
        let mut seen = HashSet::new();
        for (sc, f, _) in pool {
            if sc >= 0.45 && !seen.contains(f.path.as_str()) && related(f) {
                seen.insert(f.path.as_str());
                res.suggestions.push(publish(f, sc));
            }
            if res.suggestions.len() >= 5 {
                break;
            }
        }
        res
    }

    fn fuzzy(&self, artist: &str, title: &str, album: &str, prefer: &str) -> Scored<'_> {
        let mut q = tokens(artist);
        q.extend(tokens(title));
        if q.is_empty() {
            return vec![];
        }
        let uniq: Vec<&String> = {
            let mut s = HashSet::new();
            q.iter().filter(|t| s.insert(t.as_str())).collect()
        };
        let n = self.d.files.len().max(1) as f64;
        let mut hits: HashMap<u32, u32> = HashMap::new();
        for t in &uniq {
            if let Some(p) = self.d.index.get(t.as_str()) {
                if (p.len() as f64) < (n * 0.2).max(500.0) {
                    // ignore words that are everywhere ("the", "mix")
                    for &i in p {
                        *hits.entry(i).or_default() += 1;
                    }
                }
            }
        }
        // words that aren't in the library at all are probably typos: count close spellings instead
        for t in &uniq {
            if t.chars().count() >= 4 && !self.d.index.contains_key(t.as_str()) {
                for alt in close_matches(t, self.d.index.keys(), 3, 0.75) {
                    for &i in &self.d.index[&alt] {
                        *hits.entry(i).or_default() += 1;
                    }
                }
            }
        }
        let need = ((uniq.len() as f64 * 0.5) as u32).max(1);
        let mut counted: Vec<(u32, u32)> = hits.into_iter().collect();
        counted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let idxs: Vec<u32> = counted
            .into_iter()
            .take(4000)
            .filter(|(_, c)| *c >= need)
            .map(|(i, _)| i)
            .collect();
        if idxs.is_empty() {
            return vec![];
        }
        let mut r = self.rank(&idxs, artist, title, album, prefer);
        r.truncate(10);
        r
    }

    /// the manual search box: every file whose path contains all the words, best first
    pub fn search(&self, q: &str, prefer: &str) -> Vec<PubFile> {
        let idxs = self.candidates(&tokens(q));
        let mut out: Vec<PubFile> = if !idxs.is_empty() {
            self.rank(&idxs, "", q, "", prefer)
                .iter()
                .map(|(s, f, _)| publish(f, *s))
                .collect()
        } else {
            let r = self.match_one(
                &Request {
                    raw: q.to_string(),
                    ..Default::default()
                },
                prefer,
            );
            match r.file {
                Some(f) => std::iter::once(f).chain(r.alternatives).collect(),
                None => r.suggestions,
            }
        };
        out.truncate(30);
        out
    }
}

pub fn file_info(path: &str, root: &str) -> Track {
    let name = basename(path).to_string();
    let (stem, ext) = splitext(&name);
    let rel = if path.starts_with(root) {
        paths::relpath(path, root)
    } else {
        path.to_string()
    };
    let mut t = Track {
        path: path.to_string(),
        rel,
        title: stem.to_string(),
        ext: ext.to_lowercase().trim_start_matches('.').to_string(),
        size: paths::getsize(path).unwrap_or(0),
        name: name.clone(),
        ..Default::default()
    };
    t.fill_norms();
    t
}
