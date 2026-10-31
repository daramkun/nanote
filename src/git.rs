//! Git integration: configuring nanote as `core.editor` and recognising Git's own files.

use crate::i18n::tr;

pub fn run_git(args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn git_failed(e: String) -> String {
    format!("{}: {e}", tr("Git command failed"))
}

/// `git config --global core.editor "<this exe>" --wait`
pub fn git_set_editor() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return git_failed("current_exe".into());
    };
    let value = format!("\"{}\" --wait", exe.to_string_lossy().replace('\\', "/"));
    match run_git(&["config", "--global", "core.editor", &value]) {
        Ok(_) => format!("{} {value}", tr("Git editor set to")),
        Err(e) => git_failed(e),
    }
}

pub fn git_unset_editor() -> String {
    match run_git(&["config", "--global", "--unset", "core.editor"]) {
        Ok(_) => tr("Git editor unset").into(),
        // Exit code 5 means the key was not set; treat as already unset.
        Err(e) if e.is_empty() => tr("Git editor unset").into(),
        Err(e) => git_failed(e),
    }
}

pub fn git_show_editor() -> String {
    match run_git(&["config", "--global", "--get", "core.editor"]) {
        Ok(v) => format!("{} {v}", tr("Git editor is")),
        Err(e) if e.is_empty() => tr("Git editor is not set").into(),
        Err(e) => git_failed(e),
    }
}

/// Syntax name for well-known Git files, which have no useful extension.
pub fn git_syntax_for(file_name: &str) -> Option<&'static str> {
    Some(match file_name.to_lowercase().as_str() {
        "commit_editmsg" | "merge_msg" | "tag_editmsg" | "squash_msg" | "edit_description"
        | "notes_editmsg" => "Git Commit",
        "git-rebase-todo" => "Git Rebase Todo",
        ".gitconfig" | ".gitmodules" => "Git Config",
        ".gitignore" => "Git Ignore",
        ".gitattributes" => "Git Attributes",
        _ => return None,
    })
}
