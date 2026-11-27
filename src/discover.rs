//! Settings, and (on Windows) finding the music server's share on whatever network you're on:
//! the name/address that worked last time -> a quick scan of this network -> asks you
//! (and signs in to the share once with `net use`).

use crate::paths::{self, join};
use serde_json::{Map, Value};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

pub const SHARE_MUSIC_DEFAULT: &str = "home\\Music";

pub fn settings_file() -> String {
    join(&paths::app_dir(), "settings.json")
}

pub fn load_settings() -> Map<String, Value> {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub fn save_setting(key: &str, value: Value) {
    let mut s = load_settings();
    s.insert(key.to_string(), value);
    if let Ok(j) = serde_json::to_vec_pretty(&Value::Object(s)) {
        let _ = paths::write_atomic(&settings_file(), &j);
    }
}

pub fn setting_str(key: &str) -> Option<String> {
    load_settings()
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

pub fn unc_music(host: &str) -> String {
    let share = setting_str("share_music").unwrap_or_else(|| SHARE_MUSIC_DEFAULT.into());
    format!(
        "\\\\{host}\\{}",
        share.trim_matches(['\\', '/']).replace('/', "\\")
    )
}

pub fn port_open(host: &str, timeout: Duration) -> bool {
    let Ok(addrs) = (host, 445).to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|a| TcpStream::connect_timeout(&a, timeout).is_ok())
}

/// the /24 networks this computer is on (e.g. "192.168.1") and its own address
pub fn local_network() -> Option<(String, String)> {
    // the address used to reach other networks (no packet is actually sent)
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.254.254.254:1").ok()?;
    let ip = s.local_addr().ok()?.ip().to_string();
    if ip.starts_with("127.") || ip.starts_with("169.254.") || ip == "0.0.0.0" {
        return None;
    }
    Some((ip.rsplit_once('.')?.0.to_string(), ip))
}

/// which of these hosts answer on the file-sharing port (checked all at once)
fn open_hosts(hosts: &[String], timeout: Duration) -> Vec<String> {
    let mut open = Vec::new();
    for chunk in hosts.chunks(64) {
        let found: Vec<bool> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|h| s.spawn(move || port_open(h, timeout)))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or(false)).collect()
        });
        open.extend(
            chunk
                .iter()
                .zip(found)
                .filter(|(_, ok)| *ok)
                .map(|(h, _)| h.clone()),
        );
    }
    open
}

/// -> (host whose music share opens, best guess to sign in to)
pub fn find_server(say: impl Fn(&str), isdir: impl Fn(&str) -> bool) -> (Option<String>, String) {
    let mut names: Vec<String> = Vec::new();
    for n in [
        setting_str("server"),
        std::env::var("DUBPLATE_SERVER").ok(),
        std::env::var("SONGCOPY_SERVER").ok(), // its old name
    ]
    .into_iter()
    .flatten()
    {
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    }
    let reachable = open_hosts(&names, Duration::from_millis(600));
    for h in &reachable {
        if isdir(&unc_music(h)) {
            return (Some(h.clone()), h.clone());
        }
    }
    // reachable by NAME but not signed in yet: sign in by name, so it keeps working if the IP changes
    if let Some(n) = reachable
        .iter()
        .find(|h| !h.starts_with(|c: char| c.is_ascii_digit()))
    {
        return (None, n.clone());
    }
    say("  Not at the usual address - scanning this network for the music server ...");
    let mut open = Vec::new();
    if let Some((net, mine)) = local_network() {
        let targets: Vec<String> = (1..255)
            .map(|i| format!("{net}.{i}"))
            .filter(|h| *h != mine)
            .collect();
        open = open_hosts(&targets, Duration::from_millis(400));
        for h in &open {
            if isdir(&unc_music(h)) {
                return (Some(h.clone()), h.clone());
            }
        }
    }
    let guess = reachable
        .first()
        .or(open.first())
        .cloned()
        .unwrap_or_default();
    (None, guess)
}

/// find (and if needed sign in to) the music on the server; returns \\host\home\Music
pub fn windows_music_folder() -> String {
    use std::io::Write;
    println!("Looking for your music server ...");
    loop {
        let (host, guess) = find_server(|m| println!("{m}"), paths::isdir);
        if let Some(h) = host {
            save_setting("server", Value::from(h.clone()));
            return unc_music(&h);
        }
        println!("\n  Couldn't open the music on the server automatically.");
        println!(
            "  That's normal the first time on a new network: Windows needs you to sign in once."
        );
        println!("  Type the server's name (e.g. myserver.local, it keeps working if its address changes)");
        println!("  or its address. On a Linux server,  hostname  shows the name and  hostname -I  the address.\n");
        if guess.is_empty() {
            print!("  Server name or address: ")
        } else {
            print!("  Server name or address [Enter = {guess}]: ")
        }
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            eprintln!("No server address given.");
            std::process::exit(1);
        }
        let mut addr = line.trim().trim_matches(['\\', '/', ' ']).to_string();
        if addr.is_empty() {
            addr = guess;
        }
        if addr.is_empty() {
            continue;
        }
        if !paths::isdir(&unc_music(&addr)) {
            println!(
                "\n  Sign in with your SERVER username and password (Windows will remember it):"
            );
            let _ = std::process::Command::new("net")
                .args([
                    "use",
                    &format!("\\\\{addr}\\home"),
                    "/savecred",
                    "/persistent:yes",
                ])
                .status();
        }
        if paths::isdir(&unc_music(&addr)) {
            save_setting("server", Value::from(addr.clone()));
            return unc_music(&addr);
        }
        println!(
            "\n  Still can't open {}. Check the address and that the server is on.",
            unc_music(&addr)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_share_path() {
        let _g = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LOCALAPPDATA", tmp.path());
        assert_eq!(unc_music("x"), "\\\\x\\home\\Music");
        save_setting("server", Value::from("myserver.local"));
        assert_eq!(setting_str("server").as_deref(), Some("myserver.local"));
        save_setting("share_music", Value::from("music/"));
        assert_eq!(unc_music("x"), "\\\\x\\music");
    }
}
