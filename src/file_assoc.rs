//! File associations: nanote offered to Windows as an application for text and source files.
//!
//! Windows 10/11 does not let a program make itself the default for a file type; only the user
//! can choose that. What a program can do, without administrator rights, is register a ProgId
//! and its "capabilities" under the current user, which makes nanote appear in "Open with" and
//! in Settings → Default apps, where the file types are listed ready to be switched over.

use std::{io, path::Path};

/// The ProgId that the extensions point to.
pub const PROG_ID: &str = "nanote.File";

/// The icon that files of these types show: resource id 2 of the executable (`build.rs`).
pub const FILE_ICON_ID: u32 = 2;

/// Plain text and Markdown.
const TEXT: &[&str] = &[".txt", ".md", ".markdown"];
/// Structured data on top of [`TEXT`].
const DATA: &[&str] = &[".json", ".jsonc", ".xml", ".yaml", ".yml"];

/// Which file types are offered to Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// `.txt`, `.md`
    Text,
    /// Text plus `.json`, `.xml`, `.yml`
    Data,
    /// Everything nanote supports ([`EXTENSIONS`]).
    All,
}

impl Group {
    pub fn extensions(self) -> Vec<&'static str> {
        match self {
            Group::Text => TEXT.to_vec(),
            Group::Data => [TEXT, DATA].concat(),
            Group::All => EXTENSIONS.to_vec(),
        }
    }
}

/// Every extension nanote can be registered for.
pub const EXTENSIONS: &[&str] = &[
    ".txt",
    ".log",
    ".md",
    ".markdown",
    ".rst",
    ".json",
    ".jsonc",
    ".yaml",
    ".yml",
    ".toml",
    ".ini",
    ".cfg",
    ".conf",
    ".xml",
    ".csv",
    ".tsv",
    ".rs",
    ".py",
    ".js",
    ".mjs",
    ".ts",
    ".tsx",
    ".jsx",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".cc",
    ".cs",
    ".java",
    ".kt",
    ".go",
    ".rb",
    ".php",
    ".lua",
    ".swift",
    ".sh",
    ".bat",
    ".cmd",
    ".ps1",
    ".sql",
    ".html",
    ".htm",
    ".css",
    ".scss",
    ".diff",
    ".patch",
    ".gitignore",
    ".gitattributes",
    ".editorconfig",
    ".env",
];

/// Where the registration lives under `HKEY_CURRENT_USER`.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `Software\Classes`
    pub classes: String,
    /// The application's own key: `Software\nanote`
    pub app: String,
    /// `Software\RegisteredApplications`
    pub registered: String,
}

impl Paths {
    pub fn real() -> Self {
        Self {
            classes: r"Software\Classes".into(),
            app: r"Software\nanote".into(),
            registered: r"Software\RegisteredApplications".into(),
        }
    }

    fn capabilities(&self) -> String {
        format!(r"{}\Capabilities", self.app)
    }
}

/// What Explorer runs for a file of an associated type.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

/// Register nanote for the extensions of `group`, replacing an earlier registration (also of
/// another group).
pub fn register(exe: &Path, description: &str, group: Group) -> io::Result<()> {
    imp::register_at(&Paths::real(), exe, description, group)
}

/// Remove the registration. Not being registered is not an error.
pub fn unregister() -> io::Result<()> {
    imp::unregister_at(&Paths::real())
}

/// The program was moved: point the registration at its new place, keeping the file types.
pub fn repoint(exe: &Path) -> io::Result<()> {
    imp::repoint_at(&Paths::real(), exe)
}

/// The command that is registered, if any.
pub fn registered_command() -> Option<String> {
    imp::registered_command_at(&Paths::real())
}

/// Open Windows' "Default apps" page of nanote, where the file types can be switched over.
pub fn open_default_apps() -> io::Result<()> {
    imp::open_default_apps()
}

#[cfg(windows)]
mod imp {
    use std::{ffi::c_void, io, os::windows::process::CommandExt, path::Path, process::Command};

    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_SET_VALUE},
    };

    use super::{EXTENSIONS, FILE_ICON_ID, Group, PROG_ID, Paths, command_for};

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(event: i32, flags: u32, item1: *const c_void, item2: *const c_void);
    }

    /// Tell Explorer that associations changed, so "Open with" lists are rebuilt.
    fn notify() {
        const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, 0, std::ptr::null(), std::ptr::null()) };
    }

    pub fn registered_command_at(paths: &Paths) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!(r"{}\{PROG_ID}\shell\open\command", paths.classes))
            .ok()?
            .get_value("")
            .ok()
    }

    pub fn register_at(
        paths: &Paths,
        exe: &Path,
        description: &str,
        group: Group,
    ) -> io::Result<()> {
        // Start from nothing, so that a smaller group does not keep the types of a larger one.
        unregister_extensions(paths)?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = &paths.classes;

        // The ProgId: what "nanote" means as a file handler.
        let (prog, _) = hkcu.create_subkey(format!(r"{classes}\{PROG_ID}"))?;
        prog.set_value("", &description)?;
        prog.set_value("FriendlyTypeName", &description)?;
        let (icon, _) = prog.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{}\",-{FILE_ICON_ID}", exe.display()))?;
        let (command, _) = prog.create_subkey(r"shell\open\command")?;
        command.set_value("", &command_for(exe))?;

        let (app, _) = hkcu.create_subkey(application_key(paths, exe))?;
        write_application(&app, exe, description)?;
        let (supported, _) = app.create_subkey("SupportedTypes")?;
        for ext in group.extensions() {
            supported.set_value(ext, &"")?;
        }

        // Capabilities: what shows up in Settings → Default apps.
        let (caps, _) = hkcu.create_subkey(paths.capabilities())?;
        caps.set_value("ApplicationName", &"nanote")?;
        caps.set_value("ApplicationDescription", &description)?;
        let (assoc, _) = caps.create_subkey("FileAssociations")?;
        let (registered, _) = hkcu.create_subkey(&paths.registered)?;
        registered.set_value("nanote", &paths.capabilities())?;

        // Each extension lists nanote among its candidates ("Open with").
        for ext in group.extensions() {
            assoc.set_value(ext, &PROG_ID)?;
            let (with, _) = hkcu.create_subkey(format!(r"{classes}\{ext}\OpenWithProgids"))?;
            with.set_value(PROG_ID, &"")?;
        }
        notify();
        Ok(())
    }

    /// `Applications\<exe name>`: what Windows consults when the program was picked through
    /// "Open with" (it has no ProgId then), including the icon of the files.
    fn application_key(paths: &Paths, exe: &Path) -> String {
        let name = exe
            .file_name()
            .map_or("nanote.exe".into(), |n| n.to_string_lossy());
        format!(r"{}\Applications\{name}", paths.classes)
    }

    fn write_application(app: &RegKey, exe: &Path, description: &str) -> io::Result<()> {
        app.set_value("FriendlyAppName", &"nanote")?;
        let (icon, _) = app.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{}\",-{FILE_ICON_ID}", exe.display()))?;
        let (command, _) = app.create_subkey(r"shell\open\command")?;
        command.set_value("", &command_for(exe))?;
        let _ = description;
        Ok(())
    }

    fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
        match result {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Take nanote out of every extension and out of the capabilities' list.
    fn unregister_extensions(paths: &Paths) -> io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in EXTENSIONS {
            if let Ok(with) = hkcu.open_subkey_with_flags(
                format!(r"{}\{ext}\OpenWithProgids", paths.classes),
                winreg::enums::KEY_SET_VALUE,
            ) {
                ignore_missing(with.delete_value(PROG_ID))?;
            }
        }
        ignore_missing(
            hkcu.delete_subkey_all(format!(r"{}\FileAssociations", paths.capabilities())),
        )?;
        // The Applications key is rewritten by `register_at`; the current program and the
        // usual name are both removed here.
        for name in application_names() {
            ignore_missing(
                hkcu.delete_subkey_all(format!(r"{}\Applications\{name}", paths.classes)),
            )?;
        }
        Ok(())
    }

    fn application_names() -> Vec<String> {
        let mut names = vec!["nanote.exe".to_owned()];
        if let Some(n) = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        names
    }

    pub fn unregister_at(paths: &Paths) -> io::Result<()> {
        unregister_extensions(paths)?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(registered) =
            hkcu.open_subkey_with_flags(&paths.registered, winreg::enums::KEY_SET_VALUE)
        {
            ignore_missing(registered.delete_value("nanote"))?;
        }
        ignore_missing(hkcu.delete_subkey_all(&paths.app))?;
        ignore_missing(hkcu.delete_subkey_all(format!(r"{}\{PROG_ID}", paths.classes)))?;
        notify();
        Ok(())
    }

    pub fn repoint_at(paths: &Paths, exe: &Path) -> io::Result<()> {
        let prog = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(format!(r"{}\{PROG_ID}", paths.classes), KEY_SET_VALUE)?;
        prog.open_subkey_with_flags("DefaultIcon", KEY_SET_VALUE)?
            .set_value("", &format!("\"{}\",-{FILE_ICON_ID}", exe.display()))?;
        prog.open_subkey_with_flags(r"shell\open\command", KEY_SET_VALUE)?
            .set_value("", &command_for(exe))?;
        if let Ok(app) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(application_key(paths, exe), KEY_SET_VALUE)
        {
            write_application(&app, exe, "")?;
        }
        notify();
        Ok(())
    }

    pub fn open_default_apps() -> io::Result<()> {
        // Handed to the shell like a link. (`explorer.exe <uri>` fails with this one and opens a
        // folder window instead.)
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd.exe")
            .args(["/C", "start", "", "ms-settings:defaultapps?registeredAppUser=nanote"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::{io, path::Path};

    use super::{Group, Paths};

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "file associations are Windows only",
        )
    }

    pub fn registered_command_at(_: &Paths) -> Option<String> {
        None
    }
    pub fn register_at(_: &Paths, _: &Path, _: &str, _: Group) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn unregister_at(_: &Paths) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn repoint_at(_: &Paths, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn open_default_apps() -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::PathBuf;

    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    use super::*;

    /// Throw-away keys, so that tests never touch the real associations.
    struct TestRoot(String);

    impl TestRoot {
        fn new(name: &str) -> Self {
            Self(format!(
                r"Software\nanote-test-assoc-{name}-{}",
                std::process::id()
            ))
        }
        fn paths(&self) -> Paths {
            Paths {
                classes: format!(r"{}\Classes", self.0),
                app: format!(r"{}\App", self.0),
                registered: format!(r"{}\Registered", self.0),
            }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\nanote app\nanote.exe")
    }

    #[test]
    fn register_makes_nanote_a_candidate_for_every_extension() {
        let root = TestRoot::new("register");
        let paths = root.paths();
        assert_eq!(imp::registered_command_at(&paths), None);

        imp::register_at(&paths, &exe(), "nanote", Group::All).unwrap();
        assert_eq!(
            imp::registered_command_at(&paths),
            Some(command_for(&exe()))
        );

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in EXTENSIONS {
            let with = hkcu
                .open_subkey(format!(r"{}\{ext}\OpenWithProgids", paths.classes))
                .unwrap();
            assert!(with.get_raw_value(PROG_ID).is_ok(), "{ext}");
            let assoc: String = hkcu
                .open_subkey(format!(r"{}\FileAssociations", paths.capabilities()))
                .unwrap()
                .get_value(ext)
                .unwrap();
            assert_eq!(assoc, PROG_ID);
        }
        let registered: String = hkcu
            .open_subkey(&paths.registered)
            .unwrap()
            .get_value("nanote")
            .unwrap();
        assert_eq!(registered, paths.capabilities());
    }

    #[test]
    fn groups_register_only_their_types_and_replace_each_other() {
        let root = TestRoot::new("groups");
        let paths = root.paths();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let listed = |ext: &str| {
            hkcu.open_subkey(format!(r"{}\{ext}\OpenWithProgids", paths.classes))
                .is_ok_and(|k| k.get_raw_value(PROG_ID).is_ok())
        };
        let assoc = |ext: &str| {
            hkcu.open_subkey(format!(r"{}\FileAssociations", paths.capabilities()))
                .is_ok_and(|k| k.get_value::<String, _>(ext).is_ok())
        };

        imp::register_at(&paths, &exe(), "nanote", Group::Text).unwrap();
        for ext in [".txt", ".md"] {
            assert!(listed(ext) && assoc(ext), "{ext}");
        }
        for ext in [".json", ".xml", ".yml", ".rs"] {
            assert!(!listed(ext) && !assoc(ext), "{ext}");
        }

        imp::register_at(&paths, &exe(), "nanote", Group::Data).unwrap();
        for ext in [".txt", ".md", ".json", ".xml", ".yml", ".yaml"] {
            assert!(listed(ext) && assoc(ext), "{ext}");
        }
        assert!(!listed(".rs") && !assoc(".rs"));

        imp::register_at(&paths, &exe(), "nanote", Group::All).unwrap();
        assert!(listed(".rs") && assoc(".rs"));

        // Going back to a smaller group drops the others again.
        imp::register_at(&paths, &exe(), "nanote", Group::Text).unwrap();
        assert!(!listed(".rs") && !assoc(".rs") && !listed(".json"));
    }

    #[test]
    fn the_open_with_application_key_carries_the_file_icon() {
        let root = TestRoot::new("app");
        let paths = root.paths();
        imp::register_at(&paths, &exe(), "nanote", Group::Text).unwrap();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let app = hkcu
            .open_subkey(format!(
                r"{}\Applications\nanote.exe",
                paths.classes
            ))
            .unwrap();
        let icon: String = app
            .open_subkey("DefaultIcon")
            .unwrap()
            .get_value("")
            .unwrap();
        assert_eq!(icon, format!("\"{}\",-2", exe().display()));
        let cmd: String = app
            .open_subkey(r"shell\open\command")
            .unwrap()
            .get_value("")
            .unwrap();
        assert_eq!(cmd, command_for(&exe()));
        assert!(
            app.open_subkey("SupportedTypes")
                .unwrap()
                .get_raw_value(".md")
                .is_ok()
        );
        imp::unregister_at(&paths).unwrap();
        assert!(
            hkcu.open_subkey(format!(
                r"{}\Applications\nanote.exe",
                paths.classes
            ))
            .is_err()
        );
    }

    #[test]
    fn repoint_keeps_the_types_and_changes_the_program() {
        let root = TestRoot::new("repoint");
        let paths = root.paths();
        imp::register_at(&paths, &exe(), "nanote", Group::Text).unwrap();
        let moved = PathBuf::from(
            r"D:	ools
anote.exe",
        );
        imp::repoint_at(&paths, &moved).unwrap();
        assert_eq!(
            imp::registered_command_at(&paths),
            Some(command_for(&moved))
        );
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let icon: String = hkcu
            .open_subkey(format!(r"{}\{PROG_ID}\DefaultIcon", paths.classes))
            .unwrap()
            .get_value("")
            .unwrap();
        assert_eq!(
            icon,
            r#""D:	ools
anote.exe",-2"#
        );
        let with = hkcu
            .open_subkey(format!(r"{}\.txt\OpenWithProgids", paths.classes))
            .unwrap();
        assert!(with.get_raw_value(PROG_ID).is_ok());
    }

    #[test]
    fn every_group_is_inside_the_supported_list() {
        for group in [Group::Text, Group::Data, Group::All] {
            for ext in group.extensions() {
                assert!(EXTENSIONS.contains(&ext), "{ext}");
            }
        }
        assert!(Group::Text.extensions().len() < Group::Data.extensions().len());
        assert!(Group::Data.extensions().len() < Group::All.extensions().len());
    }

    #[test]
    fn unregister_removes_everything_and_is_repeatable() {
        let root = TestRoot::new("unregister");
        let paths = root.paths();
        imp::register_at(&paths, &exe(), "nanote", Group::All).unwrap();
        imp::unregister_at(&paths).unwrap();
        assert_eq!(imp::registered_command_at(&paths), None);

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        assert!(hkcu.open_subkey(&paths.app).is_err());
        let with = hkcu
            .open_subkey(format!(r"{}\.txt\OpenWithProgids", paths.classes))
            .unwrap();
        assert!(with.get_raw_value(PROG_ID).is_err());
        let registered = hkcu.open_subkey(&paths.registered).unwrap();
        assert!(registered.get_raw_value("nanote").is_err());
        // Nothing left to remove: still fine.
        imp::unregister_at(&paths).unwrap();
    }

    #[test]
    fn other_programs_of_an_extension_are_left_alone() {
        let root = TestRoot::new("others");
        let paths = root.paths();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (with, _) = hkcu
            .create_subkey(format!(r"{}\.txt\OpenWithProgids", paths.classes))
            .unwrap();
        with.set_value("Other.File", &"").unwrap();

        imp::register_at(&paths, &exe(), "nanote", Group::All).unwrap();
        imp::unregister_at(&paths).unwrap();
        assert!(with.get_raw_value("Other.File").is_ok());
    }
}
