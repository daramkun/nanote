//! Self-update from GitHub releases.
//!
//! The release workflow publishes `nanote.exe` and `nanote_tree_sitter.dll` as assets of
//! `v<major>.<minor>.<build>` releases. A build knows which repository it comes from because CI
//! stamps `NANOTE_REPO` at compile time; local development builds have none, so updating is off.
//!
//! Windows cannot overwrite a running executable (or a loaded DLL) but can rename it, so an
//! update downloads and verifies the new files next to the old ones, renames each old file to
//! `<name>.old` and moves the new file into its place. The new version runs after the next start.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const ASSET_EXE: &str = "nanote.exe";
pub const ASSET_DLL: &str = "nanote_tree_sitter.dll";

/// Nothing legitimate comes close to this; it only bounds a misbehaving server.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// `owner/name` this build updates from (stamped by CI). `None`: a development build.
pub fn update_repo() -> Option<&'static str> {
    option_env!("NANOTE_REPO").filter(|r| !r.is_empty())
}

/// Whether this build can update itself.
pub fn supported() -> bool {
    cfg!(windows) && update_repo().is_some()
}

// ---------------------------------------------------------------------------- version

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `0.1.42` or `v0.1.42`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().trim_start_matches('v').split('.');
        let version = Version(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(version)
    }

    /// The version this executable was built as.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

// --------------------------------------------------------------------------- releases

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Lower-case hex SHA-256 the download must match.
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub exe: Asset,
    pub dll: Option<Asset>,
}

fn api_base() -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("NANOTE_UPDATE_API_BASE") {
        return base.trim_end_matches('/').to_owned();
    }
    "https://api.github.com".to_owned()
}

/// Download URLs the updater accepts. Anything else in a release is refused.
fn trusted_prefixes(repo: &str) -> Vec<String> {
    #[allow(unused_mut)]
    let mut prefixes = vec![format!("https://github.com/{repo}/releases/download/")];
    // Development builds can be pointed at a local test server.
    #[cfg(debug_assertions)]
    if std::env::var("NANOTE_UPDATE_API_BASE").is_ok() {
        prefixes.push(format!("{}/", api_base()));
    }
    prefixes
}

/// `| `nanote.exe` | 13.7 MB | `<sha256>` |` rows of the release notes written by CI.
fn notes_hashes(notes: &str) -> Vec<(String, String)> {
    let row = Regex::new(r"\|\s*`([^`|]+)`\s*\|[^|]*\|\s*`([0-9a-fA-F]{64})`\s*\|").unwrap();
    row.captures_iter(notes)
        .map(|c| (c[1].to_owned(), c[2].to_ascii_lowercase()))
        .collect()
}

/// Turn the JSON of `GET /repos/{repo}/releases/latest` into a [`Release`].
///
/// Each asset needs a checksum: GitHub's own `digest` field, or else the table in the release
/// notes. An asset without one, or with a download URL outside `trusted`, is an error.
pub fn parse_release(json: &Value, trusted: &[String]) -> Result<Release, String> {
    let tag = json["tag_name"].as_str().ok_or("release has no tag")?;
    let version = Version::parse(tag).ok_or_else(|| format!("unrecognised release tag `{tag}`"))?;
    let hashes = notes_hashes(json["body"].as_str().unwrap_or(""));

    let asset = |name: &str| -> Result<Option<Asset>, String> {
        let Some(found) = json["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(name)))
        else {
            return Ok(None);
        };
        let url = found["browser_download_url"]
            .as_str()
            .ok_or_else(|| format!("{name}: no download URL"))?;
        if !trusted.iter().any(|prefix| url.starts_with(prefix.as_str())) {
            return Err(format!("{name}: refusing to download from {url}"));
        }
        let sha256 = found["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase)
            .or_else(|| hashes.iter().find(|(n, _)| n == name).map(|(_, h)| h.clone()))
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("{name}: the release gives no checksum"))?;
        Ok(Some(Asset {
            name: name.to_owned(),
            url: url.to_owned(),
            sha256,
        }))
    };

    Ok(Release {
        version,
        exe: asset(ASSET_EXE)?.ok_or_else(|| format!("release {tag} has no {ASSET_EXE}"))?,
        dll: asset(ASSET_DLL)?,
    })
}

// ----------------------------------------------------------------------------- network

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!("nanote/{}", Version::current()))
        .build()
        .into()
}

fn fetch_latest(repo: &str) -> Result<Release, String> {
    let url = format!("{}/repos/{repo}/releases/latest", api_base());
    let mut response = agent(Duration::from_secs(30))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("could not read the release information: {e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("bad release data: {e}"))?;
    parse_release(&json, &trusted_prefixes(repo))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `url` to `dest`, keeping it only if its SHA-256 equals `expected`.
pub fn download(url: &str, dest: &Path, expected: &str) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        let response = agent(Duration::from_secs(600))
            .get(url)
            .call()
            .map_err(|e| format!("download failed: {e}"))?;
        let mut body = response.into_body();
        let mut reader = body.with_config().limit(MAX_DOWNLOAD_BYTES).reader();
        let mut file = fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|e| format!("download failed: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n])
                .map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        file.flush().map_err(|e| e.to_string())?;
        let got = to_hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            return Err(format!("checksum mismatch (expected {expected}, got {got})"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

// ------------------------------------------------------------------------- installing

/// `<path>.old`: where a replaced file is kept until the next start.
fn old_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

fn staged_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

/// Replace each `target` with its `staged` file: the old file is renamed to `<target>.old`, the
/// new one moved into place. If any step fails, everything done so far is undone and the staged
/// files are removed, so the installation is either fully updated or untouched.
pub fn install_files(pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    // (target, whether an old file was kept at `<target>.old`)
    let mut done: Vec<(&Path, bool)> = Vec::new();
    for (target, staged) in pairs {
        let old = old_path(target);
        let step = || -> io::Result<bool> {
            if old.exists() {
                fs::remove_file(&old)?;
            }
            let had_old = target.exists();
            if had_old {
                fs::rename(target, &old)?;
            }
            if let Err(e) = fs::rename(staged, target) {
                if had_old {
                    let _ = fs::rename(&old, target);
                }
                return Err(e);
            }
            Ok(had_old)
        };
        match step() {
            Ok(had_old) => done.push((target, had_old)),
            Err(e) => {
                for (target, had_old) in done.into_iter().rev() {
                    let _ = fs::remove_file(target);
                    if had_old {
                        let _ = fs::rename(old_path(target), target);
                    }
                }
                for (_, staged) in pairs {
                    let _ = fs::remove_file(staged);
                }
                return Err(format!("could not replace {}: {e}", target.display()));
            }
        }
    }
    Ok(())
}

/// Delete the `.old` / `.new` leftovers of earlier updates next to the executable (best effort:
/// a `.old` file of a process that is still running cannot be removed yet).
pub fn cleanup_leftovers() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };
    for name in [ASSET_EXE, ASSET_DLL] {
        let path = dir.join(name);
        let _ = fs::remove_file(old_path(&path));
        let _ = fs::remove_file(staged_path(&path));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The latest release is not newer than what is installed.
    UpToDate,
    /// This version was downloaded, verified and put in place; it runs after a restart.
    Installed(Version),
}

/// Look for a release newer than `newer_than` and install it. Blocks; run it off the UI thread.
pub fn check_and_install(repo: &str, newer_than: Version) -> Result<Outcome, String> {
    let release = fetch_latest(repo)?;
    if release.version <= newer_than {
        return Ok(Outcome::UpToDate);
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the executable has no folder")?;
    let dll = dir.join(ASSET_DLL);

    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
    // The plugin is only updated when it is installed, and before the executable, so that a
    // failure part-way leaves the executable (which loads it) untouched.
    if let (true, Some(asset)) = (dll.is_file(), &release.dll) {
        let staged = staged_path(&dll);
        download(&asset.url, &staged, &asset.sha256)?;
        pairs.push((dll, staged));
    }
    let staged = staged_path(&exe);
    if let Err(e) = download(&release.exe.url, &staged, &release.exe.sha256) {
        for (_, staged) in &pairs {
            let _ = fs::remove_file(staged);
        }
        return Err(e);
    }
    pairs.push((exe, staged));

    install_files(&pairs)?;
    Ok(Outcome::Installed(release.version))
}

/// Start the freshly installed executable. It is told it is a relaunch so that it waits for this
/// process to release the single-instance port instead of starting without it.
pub fn relaunch() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command.arg("--relaunch");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: outlive this process.
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let _ = command.spawn();
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    use serde_json::json;

    use super::*;

    const HASH_A: &str = "72fa57aa74347db2198a01d97105e0e9b59a3ee861a124a130f6e6104df666e3";
    const HASH_B: &str = "e70bcaebe135dbf2f7c5910207b10992cb7b7198ee9137f6ad71cb6847955de2";

    fn trusted() -> Vec<String> {
        vec!["https://github.com/me/nanote/releases/download/".to_owned()]
    }

    fn url(name: &str) -> String {
        format!("https://github.com/me/nanote/releases/download/v0.1.9/{name}")
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nanote-updater-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn versions_parse_and_order_by_number_not_text() {
        assert_eq!(Version::parse("v0.1.42"), Some(Version(0, 1, 42)));
        assert_eq!(Version::parse("0.1.42"), Some(Version(0, 1, 42)));
        assert_eq!(Version::parse(" v1.2.3 "), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.1"), None);
        assert_eq!(Version::parse("0.1.2.3"), None);
        assert_eq!(Version::parse("v0.1.x"), None);
        assert_eq!(Version::parse("nightly"), None);
        assert!(Version(0, 1, 10) > Version(0, 1, 9)); // not "0.1.10" < "0.1.9"
        assert!(Version(0, 2, 1) > Version(0, 1, 99)); // a minor bump wins over the build number
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 1, 42).to_string(), "0.1.42");
    }

    #[test]
    fn parses_notes_written_by_the_release_workflow() {
        let notes = format!(
            "**nanote 0.1.9** (build 9)\n\n| File | Size | SHA-256 |\n|---|---|---|\n\
             | `nanote.exe` | 13.7 MB | `{HASH_A}` |\n\
             | `nanote_tree_sitter.dll` | 39.8 MB | `{}` |\n",
            HASH_B.to_uppercase()
        );
        assert_eq!(
            notes_hashes(&notes),
            vec![
                (ASSET_EXE.to_owned(), HASH_A.to_owned()),
                (ASSET_DLL.to_owned(), HASH_B.to_owned()), // lower-cased
            ]
        );
        assert!(notes_hashes("no table here").is_empty());
    }

    #[test]
    fn release_with_github_digests() {
        let json = json!({
            "tag_name": "v0.1.9",
            "body": "",
            "assets": [
                {"name": "nanote.exe", "browser_download_url": url("nanote.exe"),
                 "digest": format!("sha256:{HASH_A}")},
                {"name": "nanote_tree_sitter.dll", "browser_download_url": url("nanote_tree_sitter.dll"),
                 "digest": format!("sha256:{HASH_B}")},
                {"name": "unrelated.txt", "browser_download_url": "https://evil.example/x"},
            ]
        });
        let release = parse_release(&json, &trusted()).unwrap();
        assert_eq!(release.version, Version(0, 1, 9));
        assert_eq!(release.exe.sha256, HASH_A);
        assert_eq!(release.dll.unwrap().sha256, HASH_B);
    }

    #[test]
    fn release_falls_back_to_the_checksums_in_the_notes() {
        let json = json!({
            "tag_name": "v0.1.9",
            "body": format!("| `nanote.exe` | 1 MB | `{HASH_A}` |"),
            "assets": [{"name": "nanote.exe", "browser_download_url": url("nanote.exe")}]
        });
        let release = parse_release(&json, &trusted()).unwrap();
        assert_eq!(release.exe.sha256, HASH_A);
        assert!(release.dll.is_none()); // the plugin is optional
    }

    #[test]
    fn release_problems_are_refused() {
        let exe = |extra: Value| {
            let mut a = json!({"name": "nanote.exe", "browser_download_url": url("nanote.exe")});
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            a
        };
        let release = |tag: &str, assets: Value, body: &str| {
            json!({"tag_name": tag, "body": body, "assets": assets})
        };
        let digest = json!({"digest": format!("sha256:{HASH_A}")});

        // No checksum anywhere.
        let r = parse_release(&release("v0.1.9", json!([exe(json!({}))]), ""), &trusted());
        assert!(r.unwrap_err().contains("no checksum"));
        // A malformed checksum.
        let r = parse_release(
            &release("v0.1.9", json!([exe(json!({"digest": "sha256:abc"}))]), ""),
            &trusted(),
        );
        assert!(r.is_err());
        // A download URL outside the repository's releases.
        let r = parse_release(
            &release(
                "v0.1.9",
                json!([exe(json!({"browser_download_url": "https://evil.example/nanote.exe",
                                  "digest": format!("sha256:{HASH_A}")}))]),
                "",
            ),
            &trusted(),
        );
        assert!(r.unwrap_err().contains("refusing"));
        // A look-alike repository prefix must not pass.
        let r = parse_release(
            &release(
                "v0.1.9",
                json!([exe(json!({"browser_download_url":
                    "https://github.com/me/nanote-evil/releases/download/v1/nanote.exe",
                    "digest": format!("sha256:{HASH_A}")}))]),
                "",
            ),
            &trusted(),
        );
        assert!(r.is_err());
        // No executable asset, and an unusable tag.
        assert!(parse_release(&release("v0.1.9", json!([]), ""), &trusted()).is_err());
        assert!(parse_release(&release("latest", json!([exe(digest)]), ""), &trusted()).is_err());
    }

    #[test]
    fn install_keeps_the_old_file_and_moves_the_new_one_in() {
        let dir = temp_dir("install");
        let (exe, dll) = (dir.join("nanote.exe"), dir.join("nanote_tree_sitter.dll"));
        fs::write(&exe, "old exe").unwrap();
        fs::write(&dll, "old dll").unwrap();
        fs::write(staged_path(&exe), "new exe").unwrap();
        fs::write(staged_path(&dll), "new dll").unwrap();

        install_files(&[(dll.clone(), staged_path(&dll)), (exe.clone(), staged_path(&exe))]).unwrap();

        assert_eq!(fs::read_to_string(&exe).unwrap(), "new exe");
        assert_eq!(fs::read_to_string(&dll).unwrap(), "new dll");
        assert_eq!(fs::read_to_string(old_path(&exe)).unwrap(), "old exe");
        assert_eq!(fs::read_to_string(old_path(&dll)).unwrap(), "old dll");
        assert!(!staged_path(&exe).exists());

        // A second update replaces the stale `.old` files as well.
        fs::write(staged_path(&exe), "newer exe").unwrap();
        install_files(&[(exe.clone(), staged_path(&exe))]).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "newer exe");
        assert_eq!(fs::read_to_string(old_path(&exe)).unwrap(), "new exe");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_works_when_there_is_no_old_file() {
        let dir = temp_dir("fresh");
        let exe = dir.join("nanote.exe");
        fs::write(staged_path(&exe), "new exe").unwrap();
        install_files(&[(exe.clone(), staged_path(&exe))]).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new exe");
        assert!(!old_path(&exe).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_part_way_restores_everything() {
        let dir = temp_dir("rollback");
        let (exe, dll) = (dir.join("nanote.exe"), dir.join("nanote_tree_sitter.dll"));
        fs::write(&exe, "old exe").unwrap();
        fs::write(&dll, "old dll").unwrap();
        fs::write(staged_path(&dll), "new dll").unwrap();
        // The executable's staged file is missing, so the second step fails.

        let result =
            install_files(&[(dll.clone(), staged_path(&dll)), (exe.clone(), staged_path(&exe))]);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&exe).unwrap(), "old exe");
        assert_eq!(fs::read_to_string(&dll).unwrap(), "old dll"); // the first step was undone
        assert!(!old_path(&dll).exists());
        assert!(!staged_path(&dll).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A one-shot HTTP server answering every request with `body`.
    fn serve(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}/nanote.exe")
    }

    #[test]
    fn download_keeps_only_files_with_the_expected_checksum() {
        let dir = temp_dir("download");
        let body = b"pretend this is an executable".to_vec();
        let good = to_hex(&Sha256::digest(&body));
        let server = serve(body.clone());

        let dest = dir.join("ok.new");
        download(&server, &dest, &good).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);

        // Upper-case hex is accepted too.
        download(&server, &dest, &good.to_uppercase()).unwrap();

        let bad = dir.join("bad.new");
        let err = download(&server, &bad, HASH_A).unwrap_err();
        assert!(err.contains("checksum mismatch"), "{err}");
        assert!(!bad.exists(), "a file that failed verification must not be left behind");

        let missing = dir.join("none.new");
        assert!(download("http://127.0.0.1:1/nope", &missing, &good).is_err());
        assert!(!missing.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
