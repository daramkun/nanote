//! Path completion behind `Ctrl+P` (open a file by absolute or relative path).
//!
//! Typing a path lists the entries of its directory whose names start with what was typed after
//! the last separator. Relative paths are looked up in each of the `bases` (the folder of the
//! active file, then the working directory) and the first one where the directory exists wins.

use std::{
    fs,
    path::{MAIN_SEPARATOR, Path, PathBuf},
};

/// More than this is not useful in a list that is narrowed down by typing.
pub const MAX_RESULTS: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    File,
    /// The typed name does not exist yet: opening it starts an empty file with that path.
    NewFile,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub kind: Kind,
    /// The entry's name (for `NewFile`: the typed name).
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct Completion {
    pub candidates: Vec<Candidate>,
    /// The typed text up to and including its last separator. Choosing a folder continues from
    /// `typed_dir + name + separator`.
    pub typed_dir: String,
    /// The directory that was listed, if the typed directory part exists.
    pub dir: Option<PathBuf>,
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Replace `~` (home folder) and `%NAME%` (environment variables); unknown names stay as typed.
pub fn expand(text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let rest = match text.strip_prefix('~') {
        Some(after) if after.is_empty() || after.starts_with(is_separator) => {
            match env("USERPROFILE").or_else(|| env("HOME")) {
                Some(home) => {
                    out.push_str(&home);
                    after
                }
                None => text,
            }
        }
        _ => text,
    };

    let mut chars = rest.char_indices();
    let mut copied_to = 0;
    while let Some((i, c)) = chars.next() {
        if c != '%' {
            continue;
        }
        let Some(len) = rest[i + 1..].find('%') else {
            break;
        };
        let name = &rest[i + 1..i + 1 + len];
        if let Some(value) = (!name.is_empty()).then(|| env(name)).flatten() {
            out.push_str(&rest[copied_to..i]);
            out.push_str(&value);
            copied_to = i + len + 2;
            // Skip past the closing `%`.
            while chars.next().is_some_and(|(j, _)| j < i + len + 1) {}
        }
    }
    out.push_str(&rest[copied_to..]);
    out
}

/// What was typed, cleaned up: surrounding quotes (as produced by "Copy as path") and spaces are
/// dropped, `~` / `%VAR%` expanded, and a bare drive letter (`C:`) turned into its root.
pub fn normalize(text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut q = expand(text.trim().trim_matches('"').trim(), env);
    let bytes = q.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        q.push(MAIN_SEPARATOR);
    }
    q
}

/// The folders relative paths are resolved against: the folder of the active tab's file. Only a
/// tab without a file (not saved yet) has no such folder; then the working directory is used.
pub fn bases_for(active_file: Option<&Path>, cwd: Option<PathBuf>) -> Vec<PathBuf> {
    match active_file.and_then(Path::parent) {
        Some(dir) if !dir.as_os_str().is_empty() => vec![dir.to_path_buf()],
        _ => cwd.into_iter().collect(),
    }
}

fn split_dir(q: &str) -> (&str, &str) {
    match q.rfind(is_separator) {
        Some(i) => (&q[..=i], &q[i + 1..]),
        None => ("", q),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Names that cannot be created as files on Windows; no "new file" is offered for them.
fn is_valid_new_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['<', '>', ':', '"', '|', '?', '*'])
}

/// Candidates for the typed `text`.
pub fn complete(
    text: &str,
    bases: &[PathBuf],
    env: &dyn Fn(&str) -> Option<String>,
) -> Completion {
    let q = normalize(text, env);
    let (typed_dir, prefix) = split_dir(&q);

    let typed = Path::new(typed_dir);
    let dir: Option<PathBuf> = if typed_dir.is_empty() {
        bases.iter().find(|b| b.is_dir()).cloned()
    } else if typed.has_root() || typed.is_absolute() {
        Some(typed.to_path_buf()).filter(|p| p.is_dir())
    } else {
        bases.iter().map(|b| b.join(typed)).find(|p| p.is_dir())
    };

    let mut result = Completion {
        candidates: Vec::new(),
        typed_dir: typed_dir.to_owned(),
        dir: dir.clone(),
    };
    let Some(dir) = dir else {
        return result;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return result;
    };

    let wanted = prefix.to_lowercase();
    let show_hidden = prefix.starts_with('.');
    let mut matches: Vec<Candidate> = Vec::new();
    let mut exact = false;
    for entry in entries.flatten().take(20_000) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if !lower.starts_with(&wanted) || (name.starts_with('.') && !show_hidden) {
            continue;
        }
        let is_dir = entry
            .file_type()
            .map(|t| t.is_dir() || (t.is_symlink() && entry.path().is_dir()))
            .unwrap_or(false);
        exact |= lower == wanted;
        matches.push(Candidate {
            kind: if is_dir { Kind::Dir } else { Kind::File },
            path: absolute(&dir.join(&name)),
            name,
        });
    }

    // A name that was typed in full comes first, then folders, then files, each by name.
    matches.sort_by_cached_key(|c| {
        (
            c.name.to_lowercase() != wanted,
            c.kind != Kind::Dir,
            c.name.to_lowercase(),
        )
    });
    matches.truncate(MAX_RESULTS);
    result.candidates = matches;

    if !prefix.is_empty() && !exact && is_valid_new_name(prefix) {
        result.candidates.push(Candidate {
            kind: Kind::NewFile,
            name: prefix.to_owned(),
            path: absolute(&dir.join(prefix)),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// <tmp>/root/{alpha.txt, Beta.md, .hidden, src/{main.rs, lib.rs}, Docs/readme.md, other/x.txt}
    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nanote-quickopen-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["src", "Docs", "other"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in ["alpha.txt", "Beta.md", ".hidden", "src/main.rs", "src/lib.rs", "Docs/readme.md", "other/x.txt"] {
            fs::write(root.join(file), "x").unwrap();
        }
        root
    }

    fn names(c: &Completion) -> Vec<String> {
        c.candidates
            .iter()
            .map(|c| match c.kind {
                Kind::Dir => format!("{}/", c.name),
                Kind::File => c.name.clone(),
                Kind::NewFile => format!("+{}", c.name),
            })
            .collect()
    }

    #[test]
    fn relative_paths_start_at_the_active_tabs_file() {
        let cwd = Some(PathBuf::from("C:/work"));
        // A saved file: its folder only. The working directory must not come into play.
        assert_eq!(
            bases_for(Some(Path::new("C:/notes/today/a.txt")), cwd.clone()),
            [PathBuf::from("C:/notes/today")]
        );
        // A file in a drive root.
        assert_eq!(
            bases_for(Some(Path::new("C:/a.txt")), cwd.clone()),
            [PathBuf::from("C:/")]
        );
        // A tab that was never saved has no folder: fall back to the working directory.
        assert_eq!(bases_for(None, cwd), [PathBuf::from("C:/work")]);
        assert!(bases_for(None, None).is_empty());
    }

    #[test]
    fn empty_input_lists_the_base_folder_folders_first_hidden_excluded() {
        let root = tree("empty");
        let c = complete("", &[root.clone()], &no_env);
        assert_eq!(names(&c), ["Docs/", "other/", "src/", "alpha.txt", "Beta.md"]);
        assert_eq!(c.dir.as_deref(), Some(root.as_path()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prefix_filters_case_insensitively_and_offers_a_new_file() {
        let root = tree("prefix");
        let bases = [root.clone()];
        assert_eq!(names(&complete("al", &bases, &no_env)), ["alpha.txt", "+al"]);
        assert_eq!(names(&complete("BE", &bases, &no_env)), ["Beta.md", "+BE"]);
        // A name typed in full is not offered as new, and comes first.
        assert_eq!(names(&complete("alpha.txt", &bases, &no_env)), ["alpha.txt"]);
        // Dot files only show up when asked for.
        assert_eq!(names(&complete(".h", &bases, &no_env)), [".hidden", "+.h"]);
        assert!(!names(&complete("h", &bases, &no_env)).contains(&".hidden".to_owned()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_typed_folder_name_comes_before_longer_matches() {
        let root = tree("exact");
        fs::create_dir_all(root.join("src-extra")).unwrap();
        let c = complete("src", &[root.clone()], &no_env);
        assert_eq!(names(&c), ["src/", "src-extra/"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_paths_descend_and_support_dot_dot() {
        let root = tree("relative");
        let bases = [root.clone()];
        let c = complete("src/", &bases, &no_env);
        assert_eq!(names(&c), ["lib.rs", "main.rs"]);
        assert_eq!(c.typed_dir, "src/");
        assert_eq!(names(&complete("src\\ma", &bases, &no_env)), ["main.rs", "+ma"]);
        assert_eq!(names(&complete("src/../al", &bases, &no_env)), ["alpha.txt", "+al"]);
        let nested = [root.join("src")];
        assert_eq!(names(&complete("../Docs/", &nested, &no_env)), ["readme.md"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_first_base_where_the_folder_exists_wins() {
        let root = tree("bases");
        let elsewhere = root.join("other");
        // `src` does not exist under `other`, so the second base is used.
        let c = complete("src/", &[elsewhere.clone(), root.clone()], &no_env);
        assert_eq!(names(&c), ["lib.rs", "main.rs"]);
        // Without a typed folder the first base that exists is listed.
        assert_eq!(names(&complete("", &[elsewhere, root.clone()], &no_env)), ["x.txt"]);
        // A base that does not exist is skipped.
        let missing = root.join("nope");
        assert!(!names(&complete("", &[missing, root.clone()], &no_env)).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn absolute_paths_ignore_the_bases() {
        let root = tree("absolute");
        let query = format!("{}{}src{}", root.display(), MAIN_SEPARATOR, MAIN_SEPARATOR);
        let c = complete(&query, &[PathBuf::from("Z:/does/not/exist")], &no_env);
        assert_eq!(names(&c), ["lib.rs", "main.rs"]);
        assert!(c.candidates.iter().all(|c| c.path.is_absolute()));
        assert_eq!(c.candidates[0].path, root.join("src").join("lib.rs"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_folders_and_invalid_names_give_nothing_to_create() {
        let root = tree("missing");
        let bases = [root.clone()];
        let c = complete("nope/file", &bases, &no_env);
        assert!(c.candidates.is_empty() && c.dir.is_none());
        // Characters Windows forbids in a file name are not offered as a new file.
        assert!(complete("a?b", &bases, &no_env).candidates.is_empty());
        assert!(complete("x*", &bases, &no_env).candidates.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn quotes_and_spaces_around_a_pasted_path_are_dropped() {
        let env = no_env;
        assert_eq!(normalize("  \"C:\\Users\\me\\a b.txt\"  ", &env), "C:\\Users\\me\\a b.txt");
        assert_eq!(normalize("\"C:\\dir\\", &env), "C:\\dir\\"); // an unbalanced quote too
        assert_eq!(normalize("C:", &env), format!("C:{MAIN_SEPARATOR}"));
        assert_eq!(normalize("c:\\", &env), "c:\\");
    }

    #[test]
    fn home_and_environment_variables_expand() {
        let env = |name: &str| match name {
            "USERPROFILE" => Some("C:\\Users\\me".to_owned()),
            "APPDATA" => Some("C:\\Users\\me\\AppData\\Roaming".to_owned()),
            _ => None,
        };
        assert_eq!(expand("~", &env), "C:\\Users\\me");
        assert_eq!(expand("~\\Documents", &env), "C:\\Users\\me\\Documents");
        assert_eq!(expand("~/x", &env), "C:\\Users\\me/x");
        assert_eq!(expand("~other", &env), "~other"); // somebody else's home: left alone
        assert_eq!(expand("%APPDATA%\\nanote", &env), "C:\\Users\\me\\AppData\\Roaming\\nanote");
        assert_eq!(expand("%UNKNOWN%\\x", &env), "%UNKNOWN%\\x");
        assert_eq!(expand("100%", &env), "100%");
        assert_eq!(expand("%%", &env), "%%");
        assert_eq!(
            expand("%USERPROFILE%\\a\\%APPDATA%", &env),
            "C:\\Users\\me\\a\\C:\\Users\\me\\AppData\\Roaming"
        );
    }

    #[test]
    fn results_are_capped() {
        let root = std::env::temp_dir().join(format!("nanote-quickopen-cap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for i in 0..(MAX_RESULTS + 30) {
            fs::write(root.join(format!("f{i:04}.txt")), "x").unwrap();
        }
        let c = complete("f", &[root.clone()], &no_env);
        // The cap, plus the "new file" entry for the typed prefix.
        assert_eq!(c.candidates.len(), MAX_RESULTS + 1);
        assert_eq!(c.candidates.last().unwrap().kind, Kind::NewFile);
        let _ = fs::remove_dir_all(&root);
    }
}
