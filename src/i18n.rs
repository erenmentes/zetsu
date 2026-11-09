use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

pub fn is_en() -> bool {
    EN.load(Ordering::Relaxed)
}

pub fn set_en(v: bool) {
    EN.store(v, Ordering::Relaxed)
}

pub fn code() -> &'static str {
    if is_en() {
        "en"
    } else {
        "tr"
    }
}

pub fn init() {
    let from_cfg = crate::config::load().get("lang").cloned();
    let from_env = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX");
    match from_cfg.or(from_env) {
        Some(l) => set_en(!l.to_lowercase().starts_with("tr")),
        None => set_en(false),
    }
}

#[macro_export]
macro_rules! t {
    ($tr:expr, $en:expr) => {
        if $crate::i18n::is_en() {
            $en
        } else {
            $tr
        }
    };
}

#[macro_export]
macro_rules! tf {
    ($tr:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::i18n::is_en() {
            format!($en $(, $a)*)
        } else {
            format!($tr $(, $a)*)
        }
    };
}
