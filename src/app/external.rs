//! Files changed behind the editor's back: reading the new content, asking before overwriting
//! it, and following a file that grows (a log).

use std::path::PathBuf;

use gpui_kit::*;

use super::*;
use crate::{
    encoding::reread_file,
    filewatch::{self, Change},
};

/// Where the text ends: the last line and its length in UTF-16 units (what the editor counts).
fn end_of(text: &str) -> (u32, u32) {
    let line = text.matches('\n').count();
    let last = text.rsplit('\n').next().unwrap_or_default();
    (line as u32, last.encode_utf16().count() as u32)
}

impl NanoteApp {
    /// Has the file of tab `idx` changed on disk since it was read or written here?
    fn disk_differs(&self, idx: usize) -> bool {
        let doc = &self.docs[idx];
        let Some(path) = &doc.path else {
            return false;
        };
        let now = filewatch::stamp(path);
        match (&doc.stamp, &now) {
            (Some(seen), _) => !matches!(
                filewatch::classify(seen, now.as_ref()),
                Change::Same | Change::Gone
            ),
            // a new file that somebody else created in the meantime
            (None, now) => now.is_some(),
        }
    }

    /// Save to `path`; when the file was changed outside nanote, ask before overwriting it.
    /// `then` is told whether the file was written.
    pub fn save_guarded(
        &mut self,
        idx: usize,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, bool, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.docs[idx].path.as_deref() != Some(path.as_path()) || !self.disk_differs(idx) {
            let ok = self.write_doc(idx, path, cx);
            then(self, ok, window, cx);
            return;
        }
        let id = self.docs[idx].id;
        let name = self.docs[idx].title();
        let rx = window.prompt(
            PromptLevel::Warning,
            tr("File changed on disk"),
            Some(&format!(
                "{} {name}",
                tr("Overwrite the changes made outside nanote to:")
            )),
            &[tr("Overwrite"), tr("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = rx.await.ok();
            this.update_in(cx, |this, window, cx| {
                let Some(idx) = this.docs.iter().position(|d| d.id == id) else {
                    return;
                };
                let ok = choice == Some(0) && this.write_doc(idx, path, cx);
                then(this, ok, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Look at the files of all tabs; called regularly and when the window gets focus.
    pub(super) fn check_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for idx in 0..self.docs.len() {
            self.check_doc(idx, window, cx);
        }
    }

    fn check_doc(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let doc = &self.docs[idx];
        let Some(path) = doc.path.clone() else {
            return;
        };
        let now = filewatch::stamp(&path);
        let change = match &doc.stamp {
            Some(seen) => filewatch::classify(seen, now.as_ref()),
            None if now.is_some() => Change::Changed,
            None => Change::Same,
        };
        if matches!(change, Change::Same | Change::Gone) || doc.asked == now {
            return;
        }
        if !doc.dirty {
            let appended = change == Change::Grew && doc.follow && doc.encoding == "UTF-8";
            if appended && self.append_from_disk(idx, window, cx) {
                return;
            }
            self.reload_from_disk(idx, window, cx);
            return;
        }
        // Edited here and changed outside: the user decides, once per change.
        self.docs[idx].asked = now;
        let id = self.docs[idx].id;
        let name = self.docs[idx].title();
        let rx = window.prompt(
            PromptLevel::Warning,
            tr("File changed on disk"),
            Some(&format!(
                "{name}: {}",
                tr("it was changed outside nanote. Reload it and lose your changes?")
            )),
            &[tr("Reload"), tr("Keep mine")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                this.update_in(cx, |this, window, cx| {
                    if let Some(idx) = this.docs.iter().position(|d| d.id == id) {
                        this.reload_from_disk(idx, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Replace the text of tab `idx` by the file's current content, keeping the cursor line.
    pub(super) fn reload_from_disk(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.docs[idx].path.clone() else {
            return;
        };
        let loaded = match reread_file(&path, &self.docs[idx].encoding) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.status = format!("{}: {e}", tr("Failed to reopen")).into();
                cx.notify();
                return;
            }
        };
        let (old_text, mut cursor) = {
            let st = self.docs[idx].editor.read(cx);
            (st.value().to_string(), st.cursor_position())
        };
        let doc = &mut self.docs[idx];
        doc.crlf = loaded.crlf;
        doc.encoding = loaded.encoding;
        doc.bom = loaded.bom;
        doc.dirty = false;
        doc.stamp = loaded.stamp;
        doc.tail = loaded.tail;
        doc.asked = None;
        if old_text != loaded.text {
            // Keep the cursor where it was as far as the new text reaches.
            let lines: Vec<&str> = loaded.text.split('\n').collect();
            cursor.line = cursor.line.min(lines.len() as u32 - 1);
            let len = lines[cursor.line as usize].encode_utf16().count() as u32;
            cursor.character = cursor.character.min(len);
            doc.ignore_change = true;
            let text = loaded.text;
            doc.editor.update(cx, |st, cx| {
                st.set_value(text, window, cx);
                st.set_cursor_position(cursor, window, cx);
            });
        }
        self.status = format!("{} {}", tr("Reloaded:"), self.docs[idx].title()).into();
        cx.notify();
    }

    /// Add what was appended to the file to the end of tab `idx`. False when the file is not a
    /// continuation of what was read (the caller then reads it all again).
    fn append_from_disk(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.docs[idx].path.clone() else {
            return false;
        };
        let Ok(Some((appended, tail))) = filewatch::read_appended(&path, &self.docs[idx].tail) else {
            return false;
        };
        let doc = &mut self.docs[idx];
        doc.stamp = filewatch::stamp(&path);
        doc.tail = tail;
        doc.asked = None;
        if appended.is_empty() {
            return true;
        }
        let (old, mut cursor) = {
            let st = doc.editor.read(cx);
            (st.value().to_string(), st.cursor_position())
        };
        let (end_line, end_char) = end_of(&old);
        let at_end = cursor.line >= end_line && cursor.character >= end_char;
        let new = format!("{old}{appended}");
        if at_end {
            let (line, character) = end_of(&new);
            cursor.line = line;
            cursor.character = character;
        }
        doc.ignore_change = true;
        doc.dirty = false;
        let id = doc.id;
        doc.editor.update(cx, |st, cx| {
            st.set_value(new, window, cx);
            st.set_cursor_position(cursor, window, cx);
        });
        if at_end {
            self.show_end_soon(id, window, cx);
        }
        cx.notify();
        true
    }

    /// Put the cursor at the end of tab `id` once the editor has laid out the new text; before
    /// that it scrolls by the old height and stops a few lines short.
    fn show_end_soon(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(60))
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(doc) = this.docs.iter().find(|d| d.id == id && d.follow) else {
                    return;
                };
                let mut end = doc.editor.read(cx).cursor_position();
                let (line, character) = end_of(&doc.editor.read(cx).value().to_string());
                end.line = line;
                end.character = character;
                doc.editor
                    .update(cx, |st, cx| st.set_cursor_position(end, window, cx));
            })
            .ok();
        })
        .detach();
    }

    /// The "Follow" switch of the status bar: keep up with a file that grows, and stay at its end.
    pub fn toggle_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active;
        if self.docs[idx].path.is_none() {
            self.status = tr("Save the file first to follow its changes").into();
            cx.notify();
            return;
        }
        if self.docs[idx].follow {
            self.docs[idx].follow = false;
            cx.notify();
            return;
        }
        if self.docs[idx].dirty {
            self.status = tr("Save or reload the file first to follow its changes").into();
            cx.notify();
            return;
        }
        self.docs[idx].follow = true;
        self.check_doc(idx, window, cx);
        // Start at the end, where the new text arrives.
        let end = {
            let st = self.docs[idx].editor.read(cx);
            let mut pos = st.cursor_position();
            let (line, character) = end_of(&st.value().to_string());
            pos.line = line;
            pos.character = character;
            pos
        };
        self.docs[idx]
            .editor
            .update(cx, |st, cx| st.set_cursor_position(end, window, cx));
        let id = self.docs[idx].id;
        self.show_end_soon(id, window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::end_of;

    #[test]
    fn the_end_of_a_text_is_its_last_line_in_utf16_units() {
        assert_eq!(end_of(""), (0, 0));
        assert_eq!(end_of("abc"), (0, 3));
        assert_eq!(end_of("a\nbc\n"), (2, 0));
        assert_eq!(end_of("a\n가😀"), (1, 3)); // the emoji is two UTF-16 units
    }
}
