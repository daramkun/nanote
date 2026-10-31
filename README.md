# nanote

A fast notepad with syntax highlighting, tabs, multi-cursor editing and per-file encodings,
written in Rust with [GPUI](https://www.gpui.rs) (via GPUI Kit).

## Features

- Tabs, line numbers, word wrap, find / replace, undo / redo
- **Multi-cursor** with VS Code's shortcuts:

  | Shortcut | Action |
  |---|---|
  | `Alt+Click` | add a cursor |
  | `Ctrl+Alt+↑` / `↓` | add a cursor above / below |
  | `Ctrl+D` | select the word at the cursor, then add the next occurrence |
  | `Ctrl+K Ctrl+D` | skip the last added occurrence and select the next one |
  | `Ctrl+Shift+L` / `Ctrl+F2` | select all occurrences of the selection / current word |
  | `Shift+Alt+I` | add a cursor at the end of every selected line |
  | `Ctrl+Shift+Alt+←→↑↓`, `PageUp/PageDown` | column (box) selection with the keyboard |
  | `Alt+Shift+Drag` | column (box) selection with the mouse |
  | `Esc` | back to a single cursor |

  Not available: *Undo last cursor operation* (`Ctrl+U`) and middle-button column drag.
  `Shift+Alt+↑/↓` adds a cursor (as in the editor component) rather than copying the line.
- Syntax highlighting for ~200 languages (syntect), 32 colour themes (light/dark/system)
- Optional tree-sitter highlighting for 30+ languages through a plugin (see below)
- Per-file encoding (UTF-8/16/32, EUC-KR, Shift_JIS, GBK, Big5, Windows-125x, ...), BOM and
  LF/CRLF handling, shown and changed from the status bar (reopen / save with an encoding)
- English and Korean UI
- Font family and size, restore previously opened files on startup
- Self-updating from GitHub releases (see below)
- Single instance: `nanote file` opens a tab in the running window
  (`--new-window` / `-n` for a new window, `--reuse-window` / `-r` to force a tab)
- Git editor: `nanote --wait` blocks until the file's tab is closed
  (Settings → Git → *Set as Git editor* configures `core.editor`)

## Build

```
cargo build --release --workspace
```

This produces `target/release/nanote.exe` and `target/release/nanote_tree_sitter.dll`.
`nanote.exe` is a single, self-contained binary; the DLL is optional.

## Releases and versions

Every push to `main` runs `.github/workflows/ci.yml` on GitHub Actions: it runs the tests, builds
in release mode and publishes a GitHub release `v<major>.<minor>.<build>` with two assets,
`nanote.exe` and `nanote_tree_sitter.dll` (file names carry no version, because `nanote.exe`
looks for the DLL by name). Pull requests are tested and built but not published.

- **major.minor** come from the `VERSION` file (currently `0.1`). Edit it to change them.
- **build** is the workflow run number, so it goes up by one on every build.
- The version is stamped into both executables (file properties) during CI; the
  `version` in `Cargo.toml` is only the local-development default.

## Automatic updates

Release builds update themselves from the GitHub releases of the repository that built them
(CI stamps its name into the executable as `NANOTE_REPO`; local development builds have none,
so updating is off there).

- A check runs 8 seconds after start and then every 6 hours, in the background. Turn it off, or
  check by hand, under **Settings → General → Updates**.
- When a newer `v<major>.<minor>.<build>` exists, `nanote.exe` (and `nanote_tree_sitter.dll`, if
  it is installed next to it) are downloaded, checked against the SHA-256 published with the
  release, and swapped in. Nothing is installed if a download or checksum is wrong, and a
  failure part-way restores the previous files.
- Windows cannot overwrite a running program, so the old files are renamed to `*.old` (removed
  on the next start) and the new version runs after a restart. A dialog offers to restart right
  away; it asks about unsaved changes like any other close.
- Only downloads from `https://github.com/<repository>/releases/download/` are accepted.
  The repository must be public: private releases cannot be downloaded without credentials.
- Only the first started window checks, so several windows never update at the same time.

## tree-sitter plugin

`nanote_tree_sitter.dll` (crate `crates/nanote-tree-sitter`) bundles tree-sitter grammars and
exposes a small C ABI (`nts_*`). Put it **next to `nanote.exe`**; it is only ever loaded from
that folder. When present, the languages it supports are highlighted with tree-sitter
(Settings → Appearance → Highlighting engine); everything else, and everything when the DLL is
absent, uses the built-in syntect highlighter.

Languages: Rust, Python, C, C++, Java, C#, JavaScript (JSX), TypeScript (TSX), SQL, Go, HTML,
CSS, JSON, YAML, TOML, Markdown (with fenced-code injection), Bash, CMake, Diff, Elixir, ERB,
EJS, Kotlin, Lua, Make, PHP, Ruby, Scala, Svelte, Astro, Swift, Zig, JSDoc.

GraphQL and Protocol Buffers have tree-sitter grammars but no highlight queries, so they use
syntect.

## Command line

```
nanote [--wait] [--new-window | --reuse-window] [--] [files...]
```

## Settings and session

Stored in `%APPDATA%\nanote\settings.json` and `session.json`.
