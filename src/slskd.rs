//! Finding missing songs on Soulseek through the user's own slskd (https://github.com/slskd/slskd):
//! search, rank the results with dubplate's own matching (so it's version-aware: no radio edit
//! when you asked for the Extended Mix), queue the ones the user confirms, and follow their progress.
//! The address and API key are set by the user in Settings.

use crate::discover::{load_settings, setting_str};
use crate::library::{artist_score, parse_track, same_song, title_score};
use crate::paths::{self, SEP};
use crate::text::{first_artist, norm, round3, same_version, split_version, tokens};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const DEFAULT_URL: &str = "http://localhost:5030";

/// connection details, from Settings
pub struct Conn {
    url: String,
    key: String,
}

impl Conn {
    pub fn from_settings() -> Result<Conn, String> {
        let key = setting_str("slskd_key").ok_or(
            "Soulseek isn't set up yet: add your slskd address and API key in ⚙ Settings.",
        )?;
        let url = setting_str("slskd_url").unwrap_or_else(|| DEFAULT_URL.into());
        Ok(Conn::new(&url, &key))
    }

    pub fn new(url: &str, key: &str) -> Conn {
        Conn {
            url: url.trim().trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
        }
    }

    fn agent(timeout: u64) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(timeout))
            .build()
    }

    fn explain(&self, e: ureq::Error) -> String {
        match e {
            ureq::Error::Status(401, _) => "slskd didn't accept the API key. Check it in ⚙ Settings (and that slskd was restarted after adding it).".into(),
            ureq::Error::Status(403, _) => "slskd accepted the API key, but it's read-only. In slskd, give the key the role readwrite.".into(),
            ureq::Error::Status(404, _) => format!("{} answered, but doesn't look like slskd (or the address has an extra path).", self.url),
            ureq::Error::Status(c, r) => {
                let body = r.into_string().unwrap_or_default();
                format!("slskd answered {c}{}", if body.is_empty() { String::new() } else { format!(": {}", body.chars().take(200).collect::<String>()) })
            }
            e => format!("can't reach slskd at {} ({})", self.url, e.kind()),
        }
    }

    fn req(&self, method: &str, path: &str, timeout: u64) -> ureq::Request {
        Self::agent(timeout)
            .request(method, &format!("{}/api/v0/{path}", self.url))
            .set("X-API-Key", &self.key)
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let r = self
            .req("GET", path, 20)
            .call()
            .map_err(|e| self.explain(e))?;
        r.into_json()
            .map_err(|e| format!("slskd sent something unexpected ({e})"))
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let r = self
            .req("POST", path, 20)
            .send_json(body)
            .map_err(|e| self.explain(e))?;
        Ok(r.into_json().unwrap_or(Value::Null))
    }

    fn delete(&self, path: &str) {
        let _ = self.req("DELETE", path, 10).call();
    }

    /// for "Test connection": slskd's version, whether it's logged in to Soulseek, and where it
    /// saves downloads
    pub fn test(&self) -> Result<Value, String> {
        let app = self.get("application")?;
        let version = app
            .pointer("/version/full")
            .or(app.pointer("/version/current"))
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let logged_in = app
            .pointer("/server/isLoggedIn")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let user = app
            .pointer("/user/username")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let downloads = self.get("options").ok().and_then(|o| {
            o.pointer("/directories/downloads")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        // slskd often runs in Docker, where its folders have other names than on this computer
        let here = downloads
            .as_deref()
            .filter(|d| paths::isdir(d))
            .map(str::to_string);
        Ok(
            json!({"version": version, "logged_in": logged_in, "username": user, "downloads": downloads, "downloads_here": here}),
        )
    }

    /// search Soulseek, wait for it to finish (at most ~25 s) and return the responses
    pub fn search(&self, text: &str) -> Result<Vec<Value>, String> {
        let id = uuid4();
        // remove the search from slskd's list however this ends
        struct Tidy<'a>(&'a Conn, String);
        impl Drop for Tidy<'_> {
            fn drop(&mut self) {
                self.0.delete(&format!("searches/{}", self.1));
            }
        }
        let _tidy = Tidy(self, id.clone());
        self.post(
            "searches",
            json!({"id": id, "searchText": text, "searchTimeout": 15000, "responseLimit": 100, "fileLimit": 10000, "filterResponses": true}),
        )?;
        let start = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(
                if start.elapsed() < Duration::from_secs(3) {
                    500
                } else {
                    1000
                },
            ));
            let s = self.get(&format!("searches/{id}"))?;
            let done = s
                .get("isComplete")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || s.get("state")
                    .and_then(Value::as_str)
                    .is_some_and(|st| st.contains("Completed"));
            if done || start.elapsed() > Duration::from_secs(25) {
                break;
            }
        }
        let r = self.get(&format!("searches/{id}/responses"))?;
        Ok(r.as_array().cloned().unwrap_or_default())
    }

    /// queue downloads: [(username, filename, size)]. Every sender is asked, even if one fails;
    /// returns, per item, whether it was queued (or why not).
    pub fn download(&self, items: &[(String, String, u64)]) -> Vec<Result<(), String>> {
        let mut results: Vec<Result<(), String>> = vec![Ok(()); items.len()];
        let mut users: Vec<&str> = Vec::new();
        for (u, _, _) in items {
            if !users.contains(&u.as_str()) {
                users.push(u);
            }
        }
        for u in users {
            let idx: Vec<usize> = (0..items.len()).filter(|&i| items[i].0 == u).collect();
            let files: Vec<Value> = idx
                .iter()
                .map(|&i| json!({"filename": items[i].1, "size": items[i].2}))
                .collect();
            if let Err(e) = self.post(
                &format!("transfers/downloads/{}", encode(u)),
                Value::Array(files),
            ) {
                for i in idx {
                    results[i] = Err(e.clone());
                }
            }
        }
        results
    }

    /// every download slskd knows about: (username, filename) -> state, % done
    pub fn transfers(&self) -> Result<Vec<Transfer>, String> {
        let v = self.get("transfers/downloads")?;
        let mut out = Vec::new();
        for user in v.as_array().into_iter().flatten() {
            let username = user
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            for d in user
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for f in d
                    .get("files")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    out.push(Transfer {
                        username: username.clone(),
                        filename: f
                            .get("filename")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        state: f
                            .get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        percent: f
                            .get("percentComplete")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                        place: f.get("placeInQueue").and_then(Value::as_u64),
                    });
                }
            }
        }
        Ok(out)
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Transfer {
    pub username: String,
    pub filename: String,
    pub state: String,
    pub percent: f64,
    pub place: Option<u64>,
}

/// a file someone shares on Soulseek, ranked for one requested song
#[derive(Serialize, Clone, Debug)]
pub struct Candidate {
    pub username: String,
    pub filename: String,
    /// just the file's name, for showing
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub ext: String,
    pub bitrate: Option<u64>,
    pub bitdepth: Option<u64>,
    pub samplerate: Option<u64>,
    pub seconds: Option<u64>,
    pub free_slot: bool,
    pub speed: u64,
    pub queue: u64,
    pub score: f64,
    /// exact song (and version) - safe to preselect
    pub good: bool,
    /// the right song but another version than asked for
    pub other_version: bool,
    pub quality: String,
}

const LOSSLESS: &[&str] = &["flac", "wav", "aiff", "aif", "alac", "ape", "wv"];
const AUDIO: &[&str] = &[
    "flac", "wav", "aiff", "aif", "alac", "ape", "wv", "mp3", "m4a", "aac", "ogg", "opus", "wma",
];

/// the words to search Soulseek for: artist + title + the version's own words ("extended"),
/// without "(Original Mix)", "feat. X" and other noise that would hide good results
pub fn query(artist: &str, title: &str) -> String {
    let (base, version) = crate::text::split_version_words(title);
    let mut words = tokens(&first_artist(artist));
    words.extend(tokens(&base));
    words.extend(version); // e.g. "extended" - but not "feat X" or "original mix"
    let mut seen = std::collections::HashSet::new();
    words.retain(|w| seen.insert(w.clone()));
    words.join(" ")
}

fn quality(ext: &str, bitrate: Option<u64>, prefer: &str) -> (i32, String) {
    let lossless = LOSSLESS.contains(&ext);
    let label = if lossless {
        ext.to_uppercase()
    } else {
        match bitrate {
            Some(b) => format!("{} {b}k", ext.to_uppercase()),
            None => ext.to_uppercase(),
        }
    };
    let tier = if lossless {
        if prefer == "mp3" {
            2
        } else {
            3
        }
    } else {
        match bitrate {
            Some(b) if b >= 315 => {
                if prefer == "mp3" {
                    3
                } else {
                    2
                }
            }
            Some(b) if b >= 250 => 1,
            Some(b) if b >= 190 => 0,
            Some(_) => -1,
            None => 0,
        }
    };
    (tier, label)
}

/// rank Soulseek results for one requested song. Best first; at most `limit`.
pub fn rank(
    artist: &str,
    title: &str,
    album: &str,
    prefer: &str,
    responses: &[Value],
    limit: usize,
) -> Vec<Candidate> {
    let mut out: Vec<(Candidate, i32)> = Vec::new();
    for r in responses {
        let username = r
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if username.is_empty() {
            continue;
        }
        let free_slot = r
            .get("hasFreeUploadSlot")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let speed = r.get("uploadSpeed").and_then(Value::as_u64).unwrap_or(0);
        let queue = r.get("queueLength").and_then(Value::as_u64).unwrap_or(0);
        for f in r
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if f.get("isLocked").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let filename = f
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
            // Soulseek paths use backslashes: "@@share\Music\Artist\Album\01 - Title.flac"
            let parts: Vec<&str> = filename
                .split(['\\', '/'])
                .filter(|p| !p.is_empty())
                .collect();
            let Some(&name) = parts.last() else { continue };
            let ext = f
                .get("extension")
                .and_then(Value::as_str)
                .filter(|e| !e.is_empty())
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .unwrap_or_else(|| {
                    paths::splitext(name)
                        .1
                        .trim_start_matches('.')
                        .to_lowercase()
                });
            if !AUDIO.contains(&ext.as_str()) || size == 0 {
                continue;
            }
            let bitrate = f.get("bitRate").and_then(Value::as_u64);
            let (tier, label) = quality(&ext, bitrate, prefer);
            if tier < 0 {
                continue; // under 192 kbps: not worth a DJ's time
            }
            // read it like a library file (artist/album folders + file name)
            let tail = &parts[parts.len().saturating_sub(3)..];
            let rel = tail.join(&SEP.to_string());
            let mut t = parse_track(filename.clone(), rel, size);
            t.fill_norms();
            // the title from the file name; the artist from anywhere in the shared path, since
            // people file music as "Artist - Album/01 - Title.flac", "Artist/Album/…" or
            // "Album/01 Title.flac" (no artist at all: then it can't be preselected)
            let path_words: std::collections::HashSet<String> =
                tokens(&parts.join(" ")).into_iter().collect();
            let artist_words = tokens(&first_artist(artist));
            let s_art = if artist_words.is_empty() {
                0.8
            } else if artist_words.iter().all(|w| path_words.contains(w)) {
                1.0
            } else {
                // no sign of the artist: offered (never preselected), unless it's clearly someone else
                artist_score(&t, artist).clamp(0.4, 0.6)
            };
            let mut sc = 0.65 * title_score(&t, title) + 0.35 * s_art;
            if !album.is_empty() && norm(album) == t.n_album {
                sc = (sc + 0.05).min(1.0);
            }
            if sc < 0.75 {
                continue;
            }
            let other_version = same_song(title, &t)
                && !same_version(&split_version(title).1, &t.vtags)
                && norm(title) != t.n_title;
            let good = sc >= 0.94 && !other_version;
            out.push((
                Candidate {
                    username: username.clone(),
                    folder: parts
                        .get(parts.len().saturating_sub(2))
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    name: name.to_string(),
                    filename,
                    size,
                    ext,
                    bitrate,
                    bitdepth: f.get("bitDepth").and_then(Value::as_u64),
                    samplerate: f.get("sampleRate").and_then(Value::as_u64),
                    seconds: f.get("length").and_then(Value::as_u64),
                    free_slot,
                    speed,
                    queue,
                    score: round3(sc),
                    good,
                    other_version,
                    quality: label,
                },
                tier,
            ));
        }
    }
    // the right song and version first; then the best quality; then whoever can send it soonest
    out.sort_by(|(a, ta), (b, tb)| {
        let key = |c: &Candidate, t: &i32| {
            (
                !c.other_version,
                c.score >= 0.94,
                *t,
                c.free_slot,
                c.speed / 100_000,
                std::cmp::Reverse(c.queue),
            )
        };
        key(b, tb).cmp(&key(a, ta)).then(
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out.into_iter().take(limit).map(|(c, _)| c).collect()
}

/// a random-enough id for a search (slskd wants a GUID)
fn uuid4() -> String {
    use sha2::{Digest, Sha256};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seed = format!(
        "{:?}{}{}",
        std::time::SystemTime::now(),
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut b: [u8; 16] = Sha256::digest(seed.as_bytes())[..16].try_into().unwrap();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// percent-encode a username for a URL path
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// is Soulseek set up (an API key saved)?
pub fn configured() -> bool {
    load_settings()
        .get("slskd_key")
        .and_then(Value::as_str)
        .is_some_and(|k| !k.is_empty())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn resp(user: &str, free: bool, speed: u64, files: &[(&str, u64, Option<u64>)]) -> Value {
        json!({
            "username": user, "hasFreeUploadSlot": free, "uploadSpeed": speed, "queueLength": 0,
            "files": files.iter().map(|(f, size, br)| json!({"filename": f, "size": size, "bitRate": br, "extension": paths::splitext(f).1.trim_start_matches('.')})).collect::<Vec<_>>()
        })
    }

    #[test]
    fn search_words() {
        assert_eq!(
            query("Emptyset", "Avichi (Original Mix)"),
            "emptyset avichi"
        );
        assert_eq!(
            query("HEALTH", "In Violet (Extended Mix)"),
            "health in violet extended"
        );
        assert_eq!(
            query("Robyn feat. Someone", "Honey - Remastered 2011"),
            "robyn honey"
        );
        assert_eq!(
            query("Robyn", "Honey (feat. Kleerup) (Extended Mix)"),
            "robyn honey extended"
        );
        assert_eq!(
            query("Robyn", " Honey (Extended Mix)"),
            "robyn honey extended"
        );
    }

    #[test]
    fn ranking_is_version_aware_and_prefers_quality() {
        let rs = vec![
            resp(
                "radio",
                true,
                9_000_000,
                &[(
                    "@@a\\Music\\HEALTH\\Singles\\HEALTH - In Violet (Radio Edit).flac",
                    30_000_000,
                    None,
                )],
            ),
            resp(
                "lowq",
                true,
                9_000_000,
                &[(
                    "@@b\\HEALTH - In Violet (Extended Mix).mp3",
                    9_000_000,
                    Some(128),
                )],
            ),
            resp(
                "mp3",
                true,
                2_000_000,
                &[(
                    "@@c\\HEALTH\\In Violet\\02 - In Violet (Extended Mix).mp3",
                    15_000_000,
                    Some(320),
                )],
            ),
            resp(
                "flac",
                false,
                500_000,
                &[(
                    "@@d\\Music\\HEALTH - In Violet (Extended Mix).flac",
                    60_000_000,
                    None,
                )],
            ),
            resp(
                "other",
                true,
                9_000_000,
                &[("@@e\\Robyn - Honey.flac", 40_000_000, None)],
            ),
        ];
        let c = rank("HEALTH", "In Violet (Extended Mix)", "", "flac", &rs, 10);
        let users: Vec<&str> = c.iter().map(|c| c.username.as_str()).collect();
        assert_eq!(users[0], "flac", "{c:#?}"); // right version, lossless (even without a free slot)
        assert_eq!(users[1], "mp3");
        assert!(!users.contains(&"lowq")); // 128 kbps: dropped
        assert!(!users.contains(&"other")); // another song
        let radio = c.iter().find(|c| c.username == "radio").unwrap();
        assert!(radio.other_version && !radio.good); // a radio edit is never preselected
        assert!(c[0].good);
    }

    #[test]
    fn artist_from_anywhere_in_the_shared_path() {
        let rs = vec![
            resp(
                "a",
                true,
                1,
                &[(
                    "@@x\\Music\\Robyn - Body Talk\\01 - Honey.flac",
                    30_000_000,
                    None,
                )],
            ),
            resp(
                "b",
                true,
                1,
                &[(
                    "@@x\\Music\\Robyn\\Honey (2018)\\03 Honey.flac",
                    30_000_000,
                    None,
                )],
            ),
            resp(
                "c",
                true,
                1,
                &[("@@x\\Music\\Body Talk\\01 Honey.flac", 30_000_000, None)],
            ),
            resp(
                "d",
                true,
                1,
                &[("@@x\\Music\\Someone Else\\Honey.flac", 30_000_000, None)],
            ),
        ];
        let c = rank("Robyn", "Honey", "", "flac", &rs, 10);
        let good: Vec<&str> = c
            .iter()
            .filter(|c| c.good)
            .map(|c| c.username.as_str())
            .collect();
        assert_eq!(good, ["a", "b"], "{c:#?}"); // named by folder: found and preselected
        assert!(c.iter().any(|c| c.username == "c" && !c.good)); // no artist anywhere: offered, not preselected
        assert!(!c.iter().any(|c| c.username == "d" && c.good));
    }

    /// a fake slskd answering like the real API, for testing the whole flow without Soulseek
    pub fn fake_slskd(
        key: &'static str,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let l2 = log.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let path = req.url().to_string();
                let method = req.method().to_string();
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                l2.lock().unwrap().push(format!("{method} {path} {body}"));
                let authed = req.headers().iter().any(|h| {
                    h.field.as_str().as_str().eq_ignore_ascii_case("X-API-Key")
                        && h.value.as_str() == key
                });
                let (code, out) = if !authed {
                    (401, json!({}))
                } else if path == "/api/v0/application" {
                    (
                        200,
                        json!({"version": {"full": "0.26.2"}, "server": {"isLoggedIn": true}, "user": {"username": "me"}}),
                    )
                } else if path == "/api/v0/options" {
                    (200, json!({"directories": {"downloads": "/app/downloads"}}))
                } else if path.starts_with("/api/v0/searches") && path.ends_with("/responses") {
                    (
                        200,
                        json!([{"username": "peer one", "hasFreeUploadSlot": true, "uploadSpeed": 1000000, "queueLength": 0,
                                  "files": [{"filename": "@@x\\Music\\Robyn - Honey.flac", "size": 1234, "extension": "flac"}]}]),
                    )
                } else if path.starts_with("/api/v0/searches") && method == "GET" {
                    (
                        200,
                        json!({"isComplete": true, "state": "Completed, TimedOut"}),
                    )
                } else if path == "/api/v0/transfers/downloads" && method == "GET" {
                    (
                        200,
                        json!([{"username": "peer one", "directories": [{"directory": "@@x\\Music", "files": [
                        {"filename": "@@x\\Music\\Robyn - Honey.flac", "state": "InProgress", "percentComplete": 42.5}]}]}]),
                    )
                } else {
                    (200, json!({}))
                };
                let _ = req.respond(
                    tiny_http::Response::from_string(out.to_string()).with_status_code(code),
                );
            }
        });
        (url, log)
    }

    #[test]
    fn whole_flow_against_a_fake_slskd() {
        let (url, log) = fake_slskd("a-proper-api-key-123");
        assert!(Conn::new(&url, "wrong-key-wrong-key")
            .test()
            .unwrap_err()
            .contains("didn't accept the API key"));
        let c = Conn::new(&format!("{url}/"), "a-proper-api-key-123");
        let t = c.test().unwrap();
        assert_eq!(
            (
                t["version"].as_str(),
                t["logged_in"].as_bool(),
                t["downloads"].as_str()
            ),
            (Some("0.26.2"), Some(true), Some("/app/downloads"))
        );
        let r = c.search("robyn honey").unwrap();
        let best = rank("Robyn", "Honey", "", "flac", &r, 5);
        assert_eq!(best[0].username, "peer one");
        assert!(best[0].good);
        let r = c.download(&[(
            best[0].username.clone(),
            best[0].filename.clone(),
            best[0].size,
        )]);
        assert!(r.len() == 1 && r[0].is_ok());
        let tr = c.transfers().unwrap();
        assert_eq!((tr[0].state.as_str(), tr[0].percent), ("InProgress", 42.5));
        let log = log.lock().unwrap();
        assert!(log.iter().any(
            |l| l.starts_with("POST /api/v0/transfers/downloads/peer%20one")
                && l.contains("Robyn - Honey.flac")
        ));
        assert!(log
            .iter()
            .any(|l| l.starts_with("DELETE /api/v0/searches/"))); // searches tidied up
        assert!(Conn::new("http://127.0.0.1:9", "k")
            .test()
            .unwrap_err()
            .contains("can't reach slskd"));
    }
}
