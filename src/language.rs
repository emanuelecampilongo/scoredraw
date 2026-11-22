use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Globalization::GetUserDefaultUILanguage;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePreference {
    #[default]
    System,
    Italian,
    English,
}

// The reader also prepares diagnostics on worker threads.
static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn apply(preference: LanguagePreference) {
    const PRIMARY_LANGUAGE_MASK: u16 = 0x03ff;
    const ITALIAN: u16 = 0x10;
    let english = match preference {
        LanguagePreference::System => {
            let language = unsafe { GetUserDefaultUILanguage() };
            language & PRIMARY_LANGUAGE_MASK != ITALIAN
        }
        LanguagePreference::Italian => false,
        LanguagePreference::English => true,
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

macro_rules! tr {
    ($it:literal, $en:literal $(,)?) => {
        if crate::language::english() { $en } else { $it }
    };
}

macro_rules! trf {
    ($it:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if crate::language::english() {
            format!($en $(, $arg)*)
        } else {
            format!($it $(, $arg)*)
        }
    };
}

macro_rules! trw {
    ($it:literal, $en:literal $(,)?) => {
        if crate::language::english() {
            windows::core::w!($en)
        } else {
            windows::core::w!($it)
        }
    };
}
