use super::*;
use nyx_clipboard_core::classify::classify;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nyx-clipboard-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(t: &str) -> Captured {
    classify(vec![Rep { mime: classify::TEXT.into(), data: t.as_bytes().to_vec() }]).unwrap()
}

fn png(w: u32, h: u32, fill: u8) -> Captured {
    let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    d.extend_from_slice(&w.to_be_bytes());
    d.extend_from_slice(&h.to_be_bytes());
    d.extend(std::iter::repeat_n(fill, 100));
    classify(vec![Rep { mime: "image/png".into(), data: d }]).unwrap()
}

fn search(s: &Store, text: &str) -> Vec<String> {
    s.search(&Search { text: text.into(), ..Default::default() }).unwrap().into_iter().map(|e| e.preview).collect()
}

#[test]
fn insert_dedup_and_order() {
    let dir = tmp("dedup");
    let mut s = Store::in_memory(&dir).unwrap();
    let a = s.insert(&text("alpha"), Some("firefox"), 1.0).unwrap();
    let b = s.insert(&text("beta"), None, 2.0).unwrap();
    assert!(a.new && b.new);
    assert_eq!(search(&s, ""), vec!["beta", "alpha"]);
    let again = s.insert(&text("alpha"), None, 3.0).unwrap();
    assert_eq!(again, Inserted { id: a.id, new: false });
    assert_eq!(search(&s, ""), vec!["alpha", "beta"]);
    let e = s.entry(a.id).unwrap().unwrap();
    assert_eq!(e.uses, 2);
    assert_eq!(e.app.as_deref(), Some("firefox"));
    assert_eq!(s.stats().unwrap().entries, 2);
}

#[test]
fn search_substring_fuzzy_and_kinds() {
    let dir = tmp("search");
    let mut s = Store::in_memory(&dir).unwrap();
    s.insert(&text("The quick brown fox"), None, 1.0).unwrap();
    s.insert(&text("https://github.com/design-nexus"), None, 2.0).unwrap();
    s.insert(&text("fn main() {\n    println!(\"hi\");\n}"), None, 3.0).unwrap();
    s.insert(&png(10, 10, 1), None, 4.0).unwrap();
    assert_eq!(search(&s, "brown"), vec!["The quick brown fox"]);
    assert_eq!(search(&s, "qbf"), vec!["The quick brown fox"]);
    assert_eq!(search(&s, "url: git"), vec!["https://github.com/design-nexus"]);
    assert_eq!(search(&s, "img:"), vec!["PNG image · 10 × 10"]);
    assert_eq!(search(&s, "code: "), vec!["fn main() { …"]);
    assert!(search(&s, "zzzz").is_empty());
    let page = s.search(&Search { limit: 2, offset: 1, ..Default::default() }).unwrap();
    assert_eq!(page.len(), 2);
}

#[test]
fn pins_favorites_and_views() {
    let dir = tmp("pins");
    let mut s = Store::in_memory(&dir).unwrap();
    let a = s.insert(&text("one"), None, 1.0).unwrap();
    let b = s.insert(&text("two"), None, 2.0).unwrap();
    s.set_pinned(a.id, true, 5.0).unwrap();
    s.set_pinned(b.id, true, 6.0).unwrap();
    s.set_favorite(a.id, true).unwrap();
    let pinned: Vec<i64> = s.search(&Search { view: View::Pinned, ..Default::default() }).unwrap().iter().map(|e| e.id).collect();
    assert_eq!(pinned, vec![b.id, a.id]);
    let fav: Vec<i64> = s.search(&Search { view: View::Favorites, ..Default::default() }).unwrap().iter().map(|e| e.id).collect();
    assert_eq!(fav, vec![a.id]);
    s.clear(false).unwrap();
    assert_eq!(s.stats().unwrap().entries, 2);
    s.set_pinned(b.id, false, 7.0).unwrap();
    s.clear(false).unwrap();
    assert_eq!(s.stats().unwrap().entries, 1);
}

#[test]
fn images_live_in_files_and_go_with_their_entry() {
    let dir = tmp("images");
    let mut s = Store::in_memory(&dir).unwrap();
    let c = png(4, 3, 7);
    let i = s.insert(&c, None, 1.0).unwrap();
    let e = s.entry(i.id).unwrap().unwrap();
    let path = PathBuf::from(e.image.clone().unwrap());
    assert!(path.exists());
    assert_eq!(e.dims, Some((4, 3)));
    let reps = s.reps(i.id).unwrap();
    assert_eq!(reps[0].mime, "image/png");
    assert_eq!(reps[0].data, c.reps[0].data);
    s.delete(&[i.id]).unwrap();
    assert!(!path.exists());
}

#[test]
fn rich_text_keeps_html() {
    let dir = tmp("rich");
    let mut s = Store::in_memory(&dir).unwrap();
    let c = classify(vec![
        Rep { mime: classify::TEXT.into(), data: b"Hello".to_vec() },
        Rep { mime: classify::HTML.into(), data: b"<b>Hello</b>".to_vec() },
    ])
    .unwrap();
    let i = s.insert(&c, None, 1.0).unwrap();
    let d = s.detail(i.id).unwrap().unwrap();
    assert_eq!(d.entry.kind, Kind::Rich);
    assert_eq!(d.text, "Hello");
    assert_eq!(d.html.as_deref(), Some("<b>Hello</b>"));
    let reps = s.reps(i.id).unwrap();
    assert_eq!(reps.iter().map(|r| r.mime.as_str()).collect::<Vec<_>>(), vec![classify::TEXT, classify::HTML]);
}

#[test]
fn retention() {
    let dir = tmp("prune");
    let mut s = Store::in_memory(&dir).unwrap();
    for i in 0..10 {
        s.insert(&text(&format!("entry {i}")), None, f64::from(i) * 86_400.0).unwrap();
    }
    let keep = s.insert(&text("pinned old"), None, 0.5).unwrap();
    s.set_pinned(keep.id, true, 0.5).unwrap();
    let settings = Settings { max_entries: 5, max_age_days: 0, ..Default::default() };
    let gone = s.prune(&settings, 10.0 * 86_400.0).unwrap();
    assert_eq!(gone, 5);
    assert!(s.entry(keep.id).unwrap().is_some());
    let settings = Settings { max_entries: 0, max_age_days: 3, ..Default::default() };
    s.prune(&settings, 10.0 * 86_400.0).unwrap();
    assert_eq!(search(&s, ""), vec!["entry 9", "entry 8", "entry 7", "pinned old"]);
}

#[test]
fn snippets_and_meta() {
    let dir = tmp("snippets");
    let s = Store::in_memory(&dir).unwrap();
    let id = s.save_snippet(&Snippet { name: "Sig".into(), body: "Best, Ken".into(), ..Default::default() }, 1.0).unwrap();
    s.save_snippet(&Snippet { name: "".into(), body: "Address line".into(), ..Default::default() }, 2.0).unwrap();
    assert_eq!(s.snippets("").unwrap().len(), 2);
    assert_eq!(s.snippets("sig").unwrap()[0].id, id);
    s.save_snippet(&Snippet { id, name: "Signature".into(), body: "Thanks, Ken".into(), ..Default::default() }, 3.0).unwrap();
    assert_eq!(s.snippet(id).unwrap().unwrap().body, "Thanks, Ken");
    assert!(s.delete_snippet(id).unwrap());
    assert!(s.meta("imported").is_none());
    s.set_meta("imported", "1").unwrap();
    assert_eq!(s.meta("imported").as_deref(), Some("1"));
}

#[test]
fn file_database_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("file");
    let db = dir.join("c.db");
    let mut s = Store::open(&db, &dir.join("blobs")).unwrap();
    s.insert(&text("x"), None, 1.0).unwrap();
    assert_eq!(std::fs::metadata(&db).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(dir.join("blobs")).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn ten_thousand_entries_search_fast() {
    let dir = tmp("perf");
    let mut s = Store::in_memory(&dir).unwrap();
    for i in 0..10_000 {
        s.insert(&text(&format!("entry number {i} about item{}", i % 97)), None, f64::from(i)).unwrap();
    }
    let t = std::time::Instant::now();
    let r = s.search(&Search { text: "item42".into(), ..Default::default() }).unwrap();
    let substring = t.elapsed();
    assert!(!r.is_empty());
    let t = std::time::Instant::now();
    let _ = s.search(&Search { text: "en9".into(), ..Default::default() }).unwrap();
    let fuzzy = t.elapsed();
    let t = std::time::Instant::now();
    let _ = s.search(&Search::default()).unwrap();
    let recent = t.elapsed();
    eprintln!("search 10k: substring {substring:?}, fuzzy {fuzzy:?}, recent {recent:?}");
    // Debug builds are slow; the release budget is 16 ms.
    assert!(recent.as_millis() < 50);
}
