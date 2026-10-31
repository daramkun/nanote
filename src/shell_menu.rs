//! The Explorer context-menu entry "Open with nanote" for files.
//!
//! It is a shell verb under the current user's registry (`HKCU\Software\Classes\*\shell\nanote`),
//! so it needs no administrator rights and no installer. On Windows 11 it is shown under
//! "Show more options" (the compact menu only takes packaged shell extensions).

use std::{io, path::Path};

/// Under `HKEY_CURRENT_USER`: the verb that Explorer offers for every kind of file.
pub const KEY: &str = r"Software\Classes\*\shell\nanote";

/// What Explorer runs for the verb: the executable with the clicked file as its argument.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

/// The icon of the entry: the one embedded in the executable.
fn icon_for(exe: &Path) -> String {
    format!("\"{}\",0", exe.display())
}

/// What is currently registered.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Registered {
    pub label: String,
    pub command: String,
}

/// Add the entry (or overwrite it).
pub fn register(label: &str, exe: &Path) -> io::Result<()> {
    imp::register_at(KEY, label, exe)
}

/// Remove the entry. Not being there is not an error.
pub fn unregister() -> io::Result<()> {
    imp::unregister_at(KEY)
}

/// Make the registry say `label` / `exe`, writing only if it differs (the executable may have
/// been moved, or the interface language changed). Returns whether anything was written.
pub fn ensure(label: &str, exe: &Path) -> io::Result<bool> {
    imp::ensure_at(KEY, label, exe)
}

#[cfg(windows)]
mod imp {
    use std::{io, path::Path};

    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    use super::{Registered, command_for, icon_for};

    pub fn registered_at(key: &str) -> Option<Registered> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let verb = hkcu.open_subkey(key).ok()?;
        let label: String = verb.get_value("").ok()?;
        let command: String = verb.open_subkey("command").ok()?.get_value("").ok()?;
        Some(Registered { label, command })
    }

    pub fn register_at(key: &str, label: &str, exe: &Path) -> io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (verb, _) = hkcu.create_subkey(key)?;
        verb.set_value("", &label)?;
        verb.set_value("Icon", &icon_for(exe))?;
        let (command, _) = verb.create_subkey("command")?;
        command.set_value("", &command_for(exe))?;
        Ok(())
    }

    pub fn unregister_at(key: &str) -> io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.delete_subkey_all(key) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    pub fn ensure_at(key: &str, label: &str, exe: &Path) -> io::Result<bool> {
        let wanted = Registered {
            label: label.to_owned(),
            command: command_for(exe),
        };
        if registered_at(key).as_ref() == Some(&wanted) {
            return Ok(false);
        }
        register_at(key, label, exe)?;
        Ok(true)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::{io, path::Path};

    use super::Registered;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "the Explorer menu is Windows only")
    }

    pub fn registered_at(_: &str) -> Option<Registered> {
        None
    }
    pub fn register_at(_: &str, _: &str, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn unregister_at(_: &str) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn ensure_at(_: &str, _: &str, _: &Path) -> io::Result<bool> {
        Err(unsupported())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::PathBuf;

    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    use super::*;

    /// A throw-away key, so that tests never touch the real Explorer entry.
    struct TestKey(String);

    impl TestKey {
        fn new(name: &str) -> Self {
            let root = format!(r"Software\nanote-test-{name}-{}", std::process::id());
            Self(root)
        }
        fn verb(&self) -> String {
            format!(r"{}\shell\nanote", self.0)
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\nanote app\nanote.exe")
    }

    #[test]
    fn the_command_quotes_both_the_program_and_the_file() {
        assert_eq!(
            command_for(&exe()),
            r#""C:\Program Files\nanote app\nanote.exe" "%1""#
        );
        assert_eq!(icon_for(&exe()), r#""C:\Program Files\nanote app\nanote.exe",0"#);
    }

    #[test]
    fn register_reads_back_and_unregister_removes() {
        let key = TestKey::new("roundtrip");
        let verb = key.verb();
        assert_eq!(imp::registered_at(&verb), None);

        imp::register_at(&verb, "nanote로 열기", &exe()).unwrap();
        assert_eq!(
            imp::registered_at(&verb),
            Some(Registered {
                label: "nanote로 열기".into(),
                command: command_for(&exe()),
            })
        );
        // The icon is stored next to the label.
        let icon: String = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(&verb)
            .unwrap()
            .get_value("Icon")
            .unwrap();
        assert_eq!(icon, icon_for(&exe()));

        imp::unregister_at(&verb).unwrap();
        assert_eq!(imp::registered_at(&verb), None);
        // Removing what is not there is fine.
        imp::unregister_at(&verb).unwrap();
    }

    #[test]
    fn ensure_writes_only_when_something_differs() {
        let key = TestKey::new("ensure");
        let verb = key.verb();
        assert!(imp::ensure_at(&verb, "Open with nanote", &exe()).unwrap()); // first time
        assert!(!imp::ensure_at(&verb, "Open with nanote", &exe()).unwrap()); // nothing to do
        // The interface language changed: the label is rewritten.
        assert!(imp::ensure_at(&verb, "nanote로 열기", &exe()).unwrap());
        assert_eq!(imp::registered_at(&verb).unwrap().label, "nanote로 열기");
        // The program was moved: the command is rewritten.
        let moved = PathBuf::from(r"D:\tools\nanote.exe");
        assert!(imp::ensure_at(&verb, "nanote로 열기", &moved).unwrap());
        assert_eq!(imp::registered_at(&verb).unwrap().command, command_for(&moved));
    }

    #[test]
    fn the_real_key_is_per_user_and_covers_every_file() {
        assert!(KEY.starts_with(r"Software\Classes\*\shell\"));
    }
}
