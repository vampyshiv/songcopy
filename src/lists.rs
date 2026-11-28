//! Reading the song list: spreadsheets (.csv/.tsv/.xlsx), pasted rows, text files, .m3u playlists,
//! Traktor .nml playlists.

use crate::library::Request;
use crate::paths::{basename, splitext};
use crate::text::norm;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

const HEAD: &[(&str, &[&str])] = &[
    (
        "artist",
        &[
            "artist",
            "artists",
            "artist name",
            "artist names",
            "artist name s",
            "artist s",
            "performer",
            "band",
            "singer",
            "by",
        ],
    ),
    (
        "title",
        &[
            "title",
            "song",
            "songs",
            "track",
            "track name",
            "song name",
            "song title",
            "track title",
            "name",
            "tracks",
        ],
    ),
    ("album", &["album", "album name", "album title", "release"]),
    (
        "path",
        &[
            "path",
            "file",
            "filename",
            "file name",
            "location",
            "filepath",
            "file path",
        ],
    ),
];

static HEADER_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[,;\t]").unwrap());

pub fn read_rows(filename: &str, data: &[u8]) -> Result<Vec<Request>, String> {
    read_list(filename, data, None)
}

/// is this a Traktor file (a collection, or a playlist exported from Traktor)?
pub fn is_nml(filename: &str, data: &[u8]) -> bool {
    splitext(&filename.to_lowercase()).1 == ".nml"
        || decode(&data[..data.len().min(512)]).contains("<NML ")
}

/// read a song list; `playlist` picks one from a Traktor file that has several
/// (traktor::CHOOSE when one has to be picked)
pub fn read_list(filename: &str, data: &[u8], playlist: Option<&str>) -> Result<Vec<Request>, String> {
    if is_nml(filename, data) {
        return crate::traktor::playlist_rows(&crate::traktor::parse(&decode(data))?, playlist);
    }
    let ext = splitext(&filename.to_lowercase()).1.to_string();
    let table = match ext.as_str() {
        ".xlsx" | ".xlsm" => read_xlsx(data)?,
        ".xls" | ".numbers" | ".ods" => {
            return Err(format!(
                "{ext} files can't be read - open it and 'Save as' CSV or .xlsx"
            ));
        }
        _ => {
            let text = decode(data);
            if ext == ".m3u" || ext == ".m3u8" || text.trim_start().starts_with("#EXTM3U") {
                return Ok(read_m3u(&text));
            }
            let first = text.trim_start().split('\n').next().unwrap_or("");
            let header_like = HEADER_SPLIT.split(first).any(|c| {
                let k = norm(c);
                HEAD.iter().any(|(_, names)| names.contains(&k.as_str()))
            });
            // a spreadsheet: .csv/.tsv file, rows pasted from Excel/Sheets (tabs), or text with a header row
            if ext == ".csv"
                || ext == ".tsv"
                || first.contains('\t')
                || (header_like && (first.contains(',') || first.contains(';')))
            {
                let delim = if ext == ".tsv" || first.contains('\t') {
                    b'\t'
                } else {
                    sniff_delimiter(&text)
                };
                read_csv(&text, delim)
            } else {
                return Ok(text
                    .lines()
                    .enumerate()
                    .filter(|(_, l)| !l.trim().is_empty() && !l.trim().starts_with('#'))
                    .map(|(n, l)| Request {
                        row: Value::from(n + 1),
                        raw: l.trim().to_string(),
                        ..Default::default()
                    })
                    .collect());
            }
        }
    };
    Ok(rows_from_table(table))
}

pub fn decode(data: &[u8]) -> String {
    if let Some(rest) = data.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Ok(s) = std::str::from_utf8(data) {
        return s.to_string();
    }
    for (bom, enc) in [
        (&b"\xff\xfe"[..], encoding_rs::UTF_16LE),
        (&b"\xfe\xff"[..], encoding_rs::UTF_16BE),
    ] {
        if data.starts_with(bom) {
            return enc.decode_with_bom_removal(data).0.into_owned();
        }
    }
    encoding_rs::WINDOWS_1252.decode(data).0.into_owned()
}

/// which of , ; tab separates the columns (most consistent across the first lines)
fn sniff_delimiter(text: &str) -> u8 {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .collect();
    let mut best = (b',', 0usize);
    for d in *b",;\t" {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| l.bytes().filter(|b| *b == d).count())
            .collect();
        let Some(&first) = counts.first() else {
            continue;
        };
        if first == 0 {
            continue;
        }
        let consistent = counts.iter().filter(|&&c| c == first).count();
        if consistent > best.1 {
            best = (d, consistent);
        }
    }
    best.0
}

fn read_csv(text: &str, delim: u8) -> Vec<Vec<String>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delim)
        .from_reader(text.as_bytes());
    rdr.records()
        .flatten()
        .map(|r| r.iter().map(str::to_string).collect())
        .collect()
}

fn rows_from_table(table: Vec<Vec<String>>) -> Vec<Request> {
    let table: Vec<Vec<String>> = table
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|c| c.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .collect();
    if table.is_empty() {
        return vec![];
    }
    // find a header row in the first 5 rows
    let mut cols: Option<Vec<(&str, usize)>> = None;
    let mut start = 0;
    for (hi, r) in table.iter().take(5).enumerate() {
        let mut found: Vec<(&str, usize)> = Vec::new();
        for (ci, c) in r.iter().enumerate() {
            let k = norm(c);
            for (field, names) in HEAD {
                if names.contains(&k.as_str()) && !found.iter().any(|(f, _)| f == field) {
                    found.push((field, ci));
                }
            }
        }
        if found.iter().any(|(f, _)| *f == "title" || *f == "path") {
            cols = Some(found);
            start = hi + 1;
            break;
        }
    }
    let mut out = Vec::new();
    for (n, r) in table.iter().enumerate().skip(start) {
        let row = Value::from(n + 1);
        let req = if let Some(cols) = &cols {
            let get = |f: &str| {
                cols.iter()
                    .find(|(x, _)| *x == f)
                    .and_then(|(_, i)| r.get(*i))
                    .cloned()
                    .unwrap_or_default()
            };
            let req = Request {
                row,
                artist: get("artist"),
                title: get("title"),
                album: get("album"),
                path: get("path"),
                ..Default::default()
            };
            if req.title.is_empty() && req.path.is_empty() {
                continue;
            }
            req
        } else {
            let cells: Vec<&String> = r.iter().filter(|c| !c.is_empty()).collect();
            if cells.len() == 1 {
                Request {
                    row,
                    raw: cells[0].clone(),
                    ..Default::default()
                }
            } else {
                // no header, several columns: guess Artist, Title[, Album]
                Request {
                    row,
                    artist: cells[0].clone(),
                    title: cells[1].clone(),
                    album: cells.get(2).map(|s| s.to_string()).unwrap_or_default(),
                    ..Default::default()
                }
            }
        };
        out.push(req);
    }
    out
}

pub fn read_m3u(text: &str) -> Vec<Request> {
    let mut out = Vec::new();
    let mut extinf: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.to_uppercase().starts_with("#EXTINF") {
            extinf = line.split_once(',').map(|(_, t)| t.trim().to_string());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let line = file_uri_to_path(line);
        let line = line.as_str();
        let label = extinf
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| basename(&line.replace('\\', "/")).to_string());
        let mut req = Request {
            row: Value::from(n + 1),
            path: line.to_string(),
            label: Some(label),
            ..Default::default()
        };
        if let Some(e) = &extinf {
            if let Some((a, t)) = e.split_once(" - ") {
                req.artist = a.trim().to_string();
                req.title = t.trim().to_string();
            }
        }
        out.push(req);
        extinf = None;
    }
    out
}

/// "file:///home/me/My%20Song.mp3" (how VLC and some apps save playlists) -> a normal path
fn file_uri_to_path(line: &str) -> String {
    if line.len() < 7 || !line[..7].eq_ignore_ascii_case("file://") {
        return line.to_string();
    }
    let rest = &line[7..];
    // "file://localhost/home/x.mp3" = "file:///home/x.mp3" (the local computer)
    let rest = if rest.len() > 9 && rest[..10].eq_ignore_ascii_case("localhost/") {
        &rest[9..]
    } else {
        rest
    };
    let drive = |p: &str| {
        let b = p.as_bytes();
        b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
    };
    // file:///C:/Music/x.mp3 and file://C:/Music/x.mp3 -> C:/Music/x.mp3 ;
    // file:///home/x.mp3 -> /home/x.mp3 ; file://server/share/x -> //server/share/x
    let rest = if let Some(p) = rest.strip_prefix('/') {
        if drive(p) {
            p.to_string()
        } else {
            format!("/{p}")
        }
    } else if drive(rest) {
        rest.to_string()
    } else {
        format!("//{rest}")
    };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            ) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_xlsx(data: &[u8]) -> Result<Vec<Vec<String>>, String> {
    use calamine::{Data, Reader, Xlsx};
    let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(data.to_vec()))
        .map_err(|e| format!("not a readable .xlsx file ({e})"))?;
    let Some(Ok(range)) = wb.worksheet_range_at(0) else {
        return Ok(vec![]);
    };
    Ok(range
        .rows()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => {
                        format!("{}", *f as i64)
                    }
                    other => other.to_string(),
                })
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_csv() {
        let r = read_rows(
            "x.csv",
            b"Track Name,Artist Name(s),Album Name\nHoney,Robyn,\n",
        )
        .unwrap();
        assert_eq!(
            (r[0].artist.as_str(), r[0].title.as_str()),
            ("Robyn", "Honey")
        );
    }

    #[test]
    fn pasted_spreadsheet_rows() {
        let r = read_rows("pasted.txt", b"Robyn\tHoney\n").unwrap();
        assert_eq!(
            (r[0].artist.as_str(), r[0].title.as_str()),
            ("Robyn", "Honey")
        );
    }

    #[test]
    fn plain_lines_with_commas() {
        let r = read_rows(
            "pasted.txt",
            "Simon & Garfunkel - The Boxer, Part 2\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(r[0].raw, "Simon & Garfunkel - The Boxer, Part 2");
    }

    #[test]
    fn m3u() {
        let r = read_m3u("#EXTM3U\n#EXTINF:1,Robyn - Honey\nC:\\Music\\x.mp3\n");
        assert_eq!(
            (r[0].artist.as_str(), r[0].title.as_str()),
            ("Robyn", "Honey")
        );
        let r = read_m3u(
            "file:///home/me/A%20-%20B%C3%B6.mp3\nfile:///C:/M/x%2.mp3\nfile://nas/share/y.mp3\n",
        );
        assert_eq!(r[0].path, "/home/me/A - Bö.mp3");
        assert_eq!(r[1].path, "C:/M/x%2.mp3"); // a bad escape is left alone
        assert_eq!(r[2].path, "//nas/share/y.mp3");
        let r =
            read_m3u("file://localhost/Users/me/a.mp3\nFILE:///home/b.mp3\nfile://C:/M/c.mp3\n");
        assert_eq!(r[0].path, "/Users/me/a.mp3");
        assert_eq!(r[1].path, "/home/b.mp3");
        assert_eq!(r[2].path, "C:/M/c.mp3");
    }

    #[test]
    fn semicolon_csv_and_old_windows_encoding() {
        let r = read_rows("x.csv", b"Artist;Title\nSigur R\xf3s;Valtari\n").unwrap();
        assert_eq!(r[0].artist, "Sigur Rós");
    }
}
