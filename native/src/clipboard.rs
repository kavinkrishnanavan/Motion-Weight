//! The system clipboard.
//!
//! Windows hands the clipboard out one process at a time, so every call here
//! can legitimately fail (another app holding it for a few milliseconds is
//! normal). Failure is never worth interrupting an edit for, so it degrades to
//! "nothing happened" rather than an error.

use clipboard_win::formats;
use clipboard_win::{get_clipboard, set_clipboard};

pub fn get() -> Option<String> {
    get_clipboard(formats::Unicode).ok()
}

pub fn set(text: &str) {
    let _ = set_clipboard(formats::Unicode, text);
}
