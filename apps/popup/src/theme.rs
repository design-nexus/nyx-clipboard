//! The shared Nyx theme with the popup's sheet. It never chooses a theme: it follows
//! the one every Nyx app shares, and Omarchy's, live.

use nyx_theme::{Style, ThemeMode, ThemePrefs};

pub fn install() {
    nyx_theme::use_design_2();
    nyx_theme::install(Style {
        window_class: "nyx-clipboard",
        base_skip: &[],
        css: include_str!("style.css"),
        no_glow_css: "",
        extra_colors: None,
        prefs: || {
            let suite = nyx_theme::read_suite();
            ThemePrefs {
                mode: suite.as_ref().map(|s| s.mode.clone()).unwrap_or(ThemeMode::Omarchy),
                theme: suite.map(|s| s.theme).unwrap_or_default(),
                glow: false,
                reduce_motion: false,
            }
        },
        set_theme: None,
    });
}
