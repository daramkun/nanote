//! Noticing that a file changed on disk, and reading what was appended to it.
//!
//! Shared by the GUI and the terminal editor. Nothing here watches the file system: callers
//! ask now and then (and when their window gets focus) whether the file still looks the way it
//! was when they read it.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

/// What a file looked like when it was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

/// The file's current stamp; `None` when it does not exist (or is not a file).
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then(|| Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Same,
    /// The file is gone.
    Gone,
    /// Longer than before: probably text was appended.
    Grew,
    /// Different in some other way.
    Changed,
}

pub fn classify(seen: &Stamp, now: Option<&Stamp>) -> Change {
    match now {
        None => Change::Gone,
        Some(n) if n == seen => Change::Same,
        Some(n) if n.len > seen.len => Change::Grew,
        Some(_) => Change::Changed,
    }
}

/// How far a file was read, and the last bytes of it, to tell later whether the file is still the
/// same one with more at the end.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Tail {
    pub len: u64,
    print: Vec<u8>,
}

const PRINT: usize = 64;

impl Tail {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            print: bytes[bytes.len().saturating_sub(PRINT)..].to_vec(),
        }
    }
}

/// The text appended to a UTF-8 file since `tail`, and the new tail. `Ok(None)` means the file
/// is not simply the old one with more at the end (shortened, replaced, or changed before its
/// end), or has bytes that are not UTF-8: read it again from the start.
///
/// Only complete characters are taken, and a `\r` at the very end waits for its `\n`; what is
/// left over is read next time. Line breaks come back as `\n`, as when a file is opened.
pub fn read_appended(path: &Path, tail: &Tail) -> io::Result<Option<(String, Tail)>> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    if total < tail.len {
        return Ok(None);
    }
    let start = tail.len - tail.print.len() as u64;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let seen = tail.print.len();
    if bytes.len() < seen || bytes[..seen] != tail.print[..] {
        return Ok(None);
    }
    let new = &bytes[seen..];
    let mut valid = match std::str::from_utf8(new) {
        Ok(_) => new.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(), // a character cut at the end
        Err(_) => return Ok(None),
    };
    if new[..valid].last() == Some(&b'\r') {
        valid -= 1;
    }
    let text = std::str::from_utf8(&new[..valid]).unwrap_or_default();
    let mut all = tail.print.clone();
    all.extend_from_slice(&new[..valid]);
    let next = Tail {
        len: tail.len + valid as u64,
        print: all[all.len().saturating_sub(PRINT)..].to_vec(),
    };
    Ok(Some((text.replace("\r\n", "\n"), next)))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nanote-filewatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn classify_tells_the_kinds_of_change_apart() {
        let seen = Stamp {
            modified: Some(SystemTime::UNIX_EPOCH),
            len: 10,
        };
        assert_eq!(classify(&seen, Some(&seen)), Change::Same);
        assert_eq!(classify(&seen, None), Change::Gone);
        let grown = Stamp { len: 20, ..seen };
        assert_eq!(classify(&seen, Some(&grown)), Change::Grew);
        let shorter = Stamp { len: 5, ..seen };
        assert_eq!(classify(&seen, Some(&shorter)), Change::Changed);
        let touched = Stamp {
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            len: 10,
        };
        assert_eq!(classify(&seen, Some(&touched)), Change::Changed);
    }

    #[test]
    fn stamp_follows_the_file() {
        let path = temp("stamp.txt");
        let _ = std::fs::remove_file(&path);
        assert_eq!(stamp(&path), None);
        std::fs::write(&path, "abc").unwrap();
        assert_eq!(stamp(&path).unwrap().len, 3);
        assert_eq!(stamp(path.parent().unwrap()), None); // a folder is not a file
    }

    #[test]
    fn appended_text_is_read_from_where_the_file_was_read() {
        let path = temp("append.log");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let tail = Tail::of(&std::fs::read(&path).unwrap());
        // nothing new
        let (text, same) = read_appended(&path, &tail).unwrap().unwrap();
        assert_eq!((text.as_str(), &same), ("", &tail));
        append(&path, b"three\r\nfour\n");
        let (text, tail2) = read_appended(&path, &tail).unwrap().unwrap();
        assert_eq!(text, "three\nfour\n");
        assert_eq!(tail2.len, std::fs::metadata(&path).unwrap().len());
        // and again from the new tail
        append(&path, b"five");
        let (text, tail3) = read_appended(&path, &tail2).unwrap().unwrap();
        assert_eq!(text, "five");
        assert_eq!(tail3.len, tail2.len + 4);
    }

    #[test]
    fn a_cut_character_and_a_lone_carriage_return_wait_for_the_rest() {
        let path = temp("partial.log");
        std::fs::write(&path, "a\n").unwrap();
        let tail = Tail::of(&std::fs::read(&path).unwrap());
        let korean = "가".as_bytes(); // 3 bytes
        append(&path, &korean[..2]);
        let (text, tail2) = read_appended(&path, &tail).unwrap().unwrap();
        assert_eq!(text, "");
        assert_eq!(tail2.len, tail.len); // nothing was consumed
        append(&path, &korean[2..]);
        let (text, tail3) = read_appended(&path, &tail2).unwrap().unwrap();
        assert_eq!(text, "가");
        // a CR at the end waits for its LF, so CRLF is never split
        append(&path, b"x\r");
        let (text, tail4) = read_appended(&path, &tail3).unwrap().unwrap();
        assert_eq!(text, "x");
        append(&path, b"\ny");
        let (text, _) = read_appended(&path, &tail4).unwrap().unwrap();
        assert_eq!(text, "\ny");
    }

    #[test]
    fn a_file_that_is_not_a_continuation_asks_for_a_full_reload() {
        let path = temp("replaced.log");
        std::fs::write(&path, "the original content of the file\n").unwrap();
        let tail = Tail::of(&std::fs::read(&path).unwrap());
        // replaced by something longer that does not start the same way
        std::fs::write(&path, "completely different and a lot longer than the first one was\n").unwrap();
        assert_eq!(read_appended(&path, &tail).unwrap(), None);
        // truncated
        std::fs::write(&path, "short").unwrap();
        assert_eq!(read_appended(&path, &tail).unwrap(), None);
        // bytes that are not UTF-8
        std::fs::write(&path, "the original content of the file\n").unwrap();
        append(&path, &[0xFF, 0xFE, b'x']);
        assert_eq!(read_appended(&path, &tail).unwrap(), None);
    }
}
