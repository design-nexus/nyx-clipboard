//! Image previews, made lazily off the main thread and cached by the image's hash
//! (`~/.cache/nyx-clipboard/thumbs/<hash>-<size>.png`), so a list of screenshots costs
//! only a few small files to show.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use nyx_clipboard_core::paths;
use std::path::{Path, PathBuf};

fn cached(image: &Path, size: i32) -> Option<PathBuf> {
    let stem = image.file_stem()?.to_str()?;
    Some(paths::thumbs_dir().join(format!("{stem}-{size}.png")))
}

fn make(image: PathBuf, out: PathBuf, size: i32) -> Option<PathBuf> {
    if out.exists() {
        return Some(out);
    }
    let pix = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&image, size, size, true).ok()?;
    let _ = paths::private_dir(out.parent()?);
    pix.savev(&out, "png", &[]).ok()?;
    Some(out)
}

/// A preview of `image` (at most `size` px on its longer side), handed to `ready` once
/// it's made.
fn load_with(image: &str, size: i32, ready: impl Fn(gdk::Texture) + 'static) {
    let image = PathBuf::from(image);
    let Some(out) = cached(&image, size) else { return };
    if out.exists() {
        if let Ok(t) = gdk::Texture::from_filename(&out) {
            ready(t);
        }
        return;
    }
    glib::MainContext::default().spawn_local(async move {
        let made = gio::spawn_blocking(move || make(image, out, size)).await.ok().flatten();
        if let Some(p) = made
            && let Ok(t) = gdk::Texture::from_filename(&p)
        {
            ready(t);
        }
    });
}

/// A large preview, in a picture.
pub fn load(picture: &gtk::Picture, image: &str, size: i32) {
    let weak = picture.downgrade();
    load_with(image, size, move |t| {
        if let Some(p) = weak.upgrade() {
            p.set_paintable(Some(&t));
        }
    });
}

/// A list thumbnail, fitted into an icon-sized image.
pub fn load_icon(icon: &gtk::Image, image: &str, size: i32) {
    let weak = icon.downgrade();
    load_with(image, size, move |t| {
        if let Some(i) = weak.upgrade() {
            i.set_paintable(Some(&t));
        }
    });
}
