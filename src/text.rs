//! Text helpers: normalising names, version tags ("(Salem Remix)"), and an exact port of
//! Python's difflib similarity (SequenceMatcher.ratio / get_close_matches), which the
//! matching scores were tuned with.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use unicode_general_category::{get_general_category, GeneralCategory as G};
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;

// ---------------------------------------------------------------------------- normalising

fn is_word_char(c: char) -> bool {
    // Python's \w minus "_": letters and numbers of every alphabet
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
            | G::DecimalNumber
            | G::LetterNumber
            | G::OtherNumber
    )
}

/// lowercase, no accents, no punctuation: "Don't Stop (Björk Mix)" -> "dont stop bjork mix"
pub fn norm(s: &str) -> String {
    let s: String = s
        .nfkd()
        .filter(|c| canonical_combining_class(*c) == 0)
        .collect();
    let s = s.to_lowercase();
    let s = s
        .replace('ß', "ss")
        .replace('ø', "o")
        .replace('æ', "ae")
        .replace('đ', "d")
        .replace('ł', "l")
        .replace('&', " and ")
        .replace(['\'', '’', '‘'], "");
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if is_word_char(c) {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

pub fn tokens(s: &str) -> Vec<String> {
    norm(s).split_whitespace().map(str::to_string).collect()
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_numeric())
}

// ---------------------------------------------------------------------------- difflib port

/// Python's difflib.SequenceMatcher (isjunk=None, autojunk=True), on characters.
pub struct Matcher {
    b: Vec<char>,
    b2j: HashMap<char, Vec<usize>>,
    fullbcount: Option<HashMap<char, usize>>,
}

impl Matcher {
    pub fn new(b: &str) -> Self {
        let b: Vec<char> = b.chars().collect();
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, &c) in b.iter().enumerate() {
            b2j.entry(c).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        Matcher {
            b,
            b2j,
            fullbcount: None,
        }
    }

    fn longest(
        &self,
        a: &[char],
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ch) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(ch) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // extend over "popular" characters (autojunk), exactly like difflib
        while besti > alo && bestj > blo && a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matches(&self, a: &[char]) -> usize {
        let mut total = 0;
        let mut queue = vec![(0, a.len(), 0, self.b.len())];
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.longest(a, alo, ahi, blo, bhi);
            if k > 0 {
                total += k;
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        total
    }

    /// SequenceMatcher(None, a, b).ratio() with b = the string given to new()
    pub fn ratio(&self, a: &str) -> f64 {
        let a: Vec<char> = a.chars().collect();
        let len = a.len() + self.b.len();
        if len == 0 {
            return 1.0;
        }
        2.0 * self.matches(&a) as f64 / len as f64
    }

    fn quick_ratio(&mut self, a: &[char]) -> f64 {
        let full = self.fullbcount.get_or_insert_with(|| {
            let mut m = HashMap::new();
            for &c in &self.b {
                *m.entry(c).or_insert(0) += 1;
            }
            m
        });
        let mut avail: HashMap<char, isize> = HashMap::new();
        let mut matches = 0;
        for &c in a {
            let n = *avail
                .entry(c)
                .or_insert_with(|| *full.get(&c).unwrap_or(&0) as isize);
            avail.insert(c, n - 1);
            if n > 0 {
                matches += 1;
            }
        }
        let len = a.len() + self.b.len();
        if len == 0 {
            1.0
        } else {
            2.0 * matches as f64 / len as f64
        }
    }
}

/// SequenceMatcher(None, a, b).ratio()
pub fn ratio(a: &str, b: &str) -> f64 {
    Matcher::new(b).ratio(a)
}

/// difflib.get_close_matches(word, possibilities, n, cutoff)
pub fn close_matches<'a, I: IntoIterator<Item = &'a String>>(
    word: &str,
    possibilities: I,
    n: usize,
    cutoff: f64,
) -> Vec<String> {
    let mut m = Matcher::new(word);
    let lb = m.b.len();
    let mut result: Vec<(f64, &String)> = Vec::new();
    for x in possibilities {
        let a: Vec<char> = x.chars().collect();
        let la = a.len();
        let real_quick = if la + lb == 0 {
            1.0
        } else {
            2.0 * la.min(lb) as f64 / (la + lb) as f64
        };
        if real_quick < cutoff || m.quick_ratio(&a) < cutoff {
            continue;
        }
        let r = if la + lb == 0 {
            1.0
        } else {
            2.0 * m.matches(&a) as f64 / (la + lb) as f64
        };
        if r >= cutoff {
            result.push((r, x));
        }
    }
    // heapq.nlargest(n, (score, x)): highest score first, ties by the larger string
    result.sort_by(|p, q| q.0.partial_cmp(&p.0).unwrap().then_with(|| q.1.cmp(p.1)));
    result.into_iter().take(n).map(|(_, x)| x.clone()).collect()
}

// ---------------------------------------------------------------------------- versions

/// words in a "(...)" / " - ..." version tag that DON'T make it a different recording:
/// "(Original Mix)", "(Album Version)", "- Remastered 2011" are all just the song itself.
pub const HARMLESS: &[&str] = &[
    "remaster",
    "remastered",
    "remasters",
    "explicit",
    "clean",
    "deluxe",
    "bonus",
    "track",
    "edition",
    "mono",
    "stereo",
    "digital",
    "digitally",
    "single",
    "album",
    "version",
    "lp",
    "original",
    "the",
    "and",
    "mix",
    "dirty",
    "uncensored",
    "censored",
    "anniversary",
    "expanded",
    "official",
    "audio",
    "hq",
    "hd",
];

static HARMLESS_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HARMLESS.iter().copied().collect());
static HARMLESS_LIST: LazyLock<Vec<String>> =
    LazyLock::new(|| HARMLESS.iter().map(|s| s.to_string()).collect());

static VERSION_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*(?:[\(\[]([^\(\)\[\]]*)[\)\]]|\s[-–—]\s+([^-–—\(\)\[\]]*))\s*$").unwrap()
});
static FEAT_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(feat\.?|ft\.?|featuring|with|prod\.?)\s").unwrap());
static FEAT_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+(feat\.?|ft\.?|featuring)\s.*$").unwrap());
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\s*[\(\[][^\)\]]*(remaster|remastered|deluxe|bonus|explicit|clean|edit|version|mono|stereo|live|feat|ft\.|with )[^\)\]]*[\)\]]",
        r"|\s+-\s+.*(remaster|version|edit|mix|live|mono|stereo).*$",
        r"|\s+(feat\.?|ft\.?|featuring)\s.*$"
    ))
    .unwrap()
});
static FIRST_ARTIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[;,/]\s*|\s+(?:feat\.?|ft\.?|featuring|x|vs\.?)\s+").unwrap()
});

static HARMLESS_CACHE: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);
static SPLIT_CACHE: LazyLock<Mutex<HashMap<String, (String, String)>>> =
    LazyLock::new(Default::default);

/// is this word in a version tag harmless? ("Rmeastered" is still "Remastered")
pub fn harmless(w: &str) -> bool {
    if HARMLESS_SET.contains(w) || is_digits(w) {
        return true;
    }
    if let Some(&v) = HARMLESS_CACHE.lock().unwrap().get(w) {
        return v;
    }
    let v = char_len(w) >= 6 && !close_matches(w, HARMLESS_LIST.iter(), 1, 0.85).is_empty();
    HARMLESS_CACHE.lock().unwrap().insert(w.to_string(), v);
    v
}

/// "In Violet (Salem Remix)" -> ("In Violet", "salemremix");  "Avichi (Original Mix)" -> ("Avichi", "")
/// The tag is the non-harmless words run together, so "Nobu bros" = "Nobubros".
pub fn split_version(title: &str) -> (String, String) {
    if let Some(v) = SPLIT_CACHE.lock().unwrap().get(title) {
        return v.clone();
    }
    let (base, words) = split_version_words(title);
    let v = (base, words.concat());
    let mut cache = SPLIT_CACHE.lock().unwrap();
    if cache.len() > 200_000 {
        cache.clear();
    }
    cache.insert(title.to_string(), v.clone());
    v
}

/// the title without its version tags, and the version's own words in order
/// ("Honey (feat. X) (Extended Mix)" -> ("Honey", ["extended"]))
pub fn split_version_words(title: &str) -> (String, Vec<String>) {
    let mut t = title.to_string();
    let mut tags: Vec<String> = Vec::new();
    while let Some(c) = VERSION_TAIL.captures(&t) {
        let m = c.get(0).unwrap();
        if m.start() == 0 {
            break;
        }
        let tag = c
            .get(1)
            .or_else(|| c.get(2))
            .map(|g| g.as_str().to_string())
            .unwrap_or_default();
        tags.push(tag);
        t.truncate(m.start());
    }
    let mut words = Vec::new();
    for tag in tags.iter().rev() {
        if FEAT_TAG.is_match(tag) {
            continue; // "(feat. X)" is not a different version
        }
        words.extend(tokens(tag).into_iter().filter(|w| !harmless(w)));
    }
    (FEAT_TAIL.replace(&t, "").trim().to_string(), words)
}

pub fn same_version(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if ratio(a, b) >= 0.85 {
        return true;
    }
    // "RAC mix" typed as "RCA mix": same letters, swapped
    let (mut x, mut y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    x.sort_unstable();
    y.sort_unstable();
    x.len() >= 3 && x == y
}

/// "Song - Remastered 2011" / "Song (feat. X)" -> "Song"
pub fn clean_title(t: &str) -> String {
    let c = NOISE.replace_all(t, "").trim().to_string();
    if c.is_empty() {
        t.to_string()
    } else {
        c
    }
}

pub fn first_artist(a: &str) -> String {
    FIRST_ARTIST
        .split(a)
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Python's round(x, 3)
pub fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalising() {
        assert_eq!(norm("Don't Stop (Björk Mix)"), "dont stop bjork mix");
        assert_eq!(norm("Sigur Rós & Friends"), "sigur ros and friends");
        assert_eq!(norm("Straße"), "strasse");
        assert_eq!(norm("  日本語 / кириллица_x "), "日本語 кириллица x");
    }

    #[test]
    fn difflib_matches_python() {
        // values from Python's difflib
        assert!((ratio("wrold in my eyes", "world in my eyes") - 0.9375).abs() < 1e-9);
        assert!((ratio("abcd", "bcde") - 0.75).abs() < 1e-9);
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(
            close_matches(
                "appel",
                ["ape", "apple", "peach", "puppy"].map(String::from).iter(),
                3,
                0.6
            ),
            ["apple", "ape"]
        );
    }

    #[test]
    fn versions() {
        assert_eq!(
            split_version("Avichi (Original Mix)"),
            ("Avichi".into(), "".into())
        );
        assert_eq!(
            split_version("In Violet - Salem Remix"),
            ("In Violet".into(), "salemremix".into())
        );
        assert_eq!(
            split_version("Song (feat. X) [Extended Mix]"),
            ("Song".into(), "extended".into())
        );
        assert_eq!(
            split_version("(I Just) Died in Your Arms").0,
            "(I Just) Died in Your Arms"
        );
        assert_eq!(
            split_version("Song - Remastered 2011"),
            ("Song".into(), "".into())
        );
        assert!(harmless("rmeastered"));
        assert!(same_version("rca", "rac"));
        assert!(!same_version("club", ""));
        assert_eq!(
            clean_title("Don't Want You Back - Remastered 2011"),
            "Don't Want You Back"
        );
        assert_eq!(first_artist("LORN;Someone Else"), "LORN");
    }
}
