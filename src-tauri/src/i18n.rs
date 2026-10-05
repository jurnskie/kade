//! Backend text in the user's language (English or Dutch).
//!
//! The frontend owns the choice (Settings → Language) and tells the backend
//! with `set_language`; until then the system locale decides.

use std::sync::atomic::{AtomicBool, Ordering};

static DUTCH: AtomicBool = AtomicBool::new(false);

pub fn nl() -> bool {
    DUTCH.load(Ordering::Relaxed)
}

/// "nl", "en", or anything else to follow the system locale.
pub fn set(lang: &str) {
    let dutch = match lang {
        "nl" => true,
        "en" => false,
        _ => system_is_dutch(),
    };
    DUTCH.store(dutch, Ordering::Relaxed);
}

fn system_is_dutch() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .is_some_and(|v| v.to_lowercase().starts_with("nl"))
}

/// `tr!("English {x}", "Nederlands {x}", x = value)`: a `String` in the
/// current language. Arguments must be passed explicitly (named or positional).
#[macro_export]
macro_rules! tr {
    ($en:literal, $nl:literal $(, $($args:tt)*)?) => {
        if $crate::i18n::nl() {
            format!($nl $(, $($args)*)?)
        } else {
            format!($en $(, $($args)*)?)
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_language() {
        super::set("nl");
        assert_eq!(tr!("{n} files", "{n} bestanden", n = 3), "3 bestanden");
        super::set("en");
        assert_eq!(tr!("{n} files", "{n} bestanden", n = 3), "3 files");
    }
}
