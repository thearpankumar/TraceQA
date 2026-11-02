//! The rule of `state.rs` tried on ids the engine really found, in `tests/data/screen_ids.json`.
//!
//! `scripts/capture_screen_ids.py` made that file from the Buggy Shop (at two sizes, and twice at one) and the pages of
//! `testbed/sites/patterns`, in Chromium. The numbers in these tests are what was measured there, and each says so,
//! so that a change to the rule that moves one of them is noticed and looked at, not tuned away.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::state::{chrome_ids, Look, Screens, Similarity, DEFAULT_CHROME_SHARE, DEFAULT_SIMILARITY};

struct Page {
    url: String,
    all: BTreeSet<String>,
    visible: BTreeSet<String>,
}

fn pages() -> (u64, BTreeMap<String, Page>) {
    let file: Value = serde_json::from_str(include_str!("../tests/data/screen_ids.json")).unwrap();
    let set = |value: &Value| -> BTreeSet<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_string())
            .collect()
    };
    let pages = file["pages"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, page)| {
            (
                name.clone(),
                Page {
                    url: page["url"].as_str().unwrap().to_string(),
                    all: set(&page["all"]),
                    visible: set(&page["visible"]),
                },
            )
        })
        .collect();
    (file["fingerprintVersion"].as_u64().unwrap(), pages)
}

fn look(page: &Page, ids: &BTreeSet<String>) -> Look {
    Look {
        url: page.url.clone(),
        ids: ids.clone(),
        modal: None,
        frames: BTreeSet::new(),
    }
}

fn screens(share: f64) -> Screens {
    Screens::new(Similarity::new(share).unwrap(), BTreeSet::new())
}

#[test]
fn the_ids_were_made_by_the_fingerprint_this_build_has() {
    let (version, pages) = pages();
    assert_eq!(
        version,
        crate::fingerprint::VERSION,
        "the ids in tests/data/screen_ids.json are from another fingerprint version: run `uv run scripts/capture_screen_ids.py`"
    );
    for (name, page) in &pages {
        assert!(!page.visible.is_empty(), "{name} has no visible component");
        assert!(page.visible.is_subset(&page.all), "{name}");
        for id in &page.all {
            assert!(
                id.len() == 14 && id.starts_with("c_") && id[2..].chars().all(|c| c.is_ascii_hexdigit()),
                "{name}: {id}"
            );
        }
    }
}

#[test]
fn the_shop_read_twice_is_one_screen() {
    // Measured: the two readings have the same 35 visible ids.
    let (_, pages) = pages();
    let (first, again) = (&pages["shop-desktop"], &pages["shop-desktop-again"]);
    assert_eq!(first.visible, again.visible);
    let mut s = screens(0.9);
    let a = s.observe(&look(first, &first.visible)).unwrap();
    let b = s.observe(&look(again, &again.visible)).unwrap();
    assert_eq!((a.id, b.is_new), (b.id, false));
    assert_eq!(s.get(a.id).unwrap().looks, 2);
}

#[test]
fn two_hidden_controls_appearing_do_not_make_a_new_screen() {
    // Measured: the shop has 37 components, 35 of them visible, so a look with and without the two hidden ones shares
    // 35 of 37 ids (94.6%). It is the real case of a banner closing: a few controls come and go on a full page.
    let (_, pages) = pages();
    let shop = &pages["shop-desktop"];
    assert_eq!((shop.visible.len(), shop.all.len()), (35, 37));
    let mut s = screens(0.9);
    s.observe(&look(shop, &shop.visible)).unwrap();
    assert!(!s.observe(&look(shop, &shop.all)).unwrap().is_new);
    // At 100% it would be two.
    let mut strict = screens(1.0);
    strict.observe(&look(shop, &shop.visible)).unwrap();
    assert!(strict.observe(&look(shop, &shop.all)).unwrap().is_new);
}

#[test]
fn the_phone_layout_of_the_shop_is_another_screen_at_90_percent_and_the_same_at_85() {
    // Measured: the desktop and the phone show 35 and 32 visible controls and share 31 of 36 ids (86.1%), because
    // the phone has a menu button and drops some of the desktop's links. This is where the share matters: it is a
    // setting (`--state-similarity`), and this is the kind of difference that shows what it should be.
    let (_, pages) = pages();
    let (desktop, mobile) = (&pages["shop-desktop"], &pages["shop-mobile"]);
    let both = desktop.visible.intersection(&mobile.visible).count();
    let either = desktop.visible.union(&mobile.visible).count();
    assert_eq!((both, either), (31, 36));

    let mut default = Screens::new(DEFAULT_SIMILARITY, BTreeSet::new());
    default.observe(&look(desktop, &desktop.visible)).unwrap();
    assert!(default.observe(&look(mobile, &mobile.visible)).unwrap().is_new);

    let mut loose = screens(0.85);
    loose.observe(&look(desktop, &desktop.visible)).unwrap();
    assert!(!loose.observe(&look(mobile, &mobile.visible)).unwrap().is_new);
}

#[test]
fn every_pattern_page_is_its_own_screen() {
    // Measured: no two of the nine pages share a single id, and each has its own address.
    let (_, pages) = pages();
    let mut s = screens(0.9);
    let mut seen = 0;
    for (name, page) in pages.iter().filter(|(name, _)| name.starts_with("pattern-")) {
        let result = s.observe(&look(page, &page.visible)).unwrap();
        assert!(result.is_new, "{name}");
        assert_eq!(result.id.index(), seen, "{name}");
        seen += 1;
    }
    assert_eq!(seen, 9);
    assert_eq!(s.len(), 9);
}

#[test]
fn a_menu_that_opens_is_another_screen() {
    // Measured: the mega menu page has 10 components and 5 are visible while its panels are closed. With a panel
    // open the others appear: 5 of 10 ids shared (50%), well under the share, so it is a new screen at the same address.
    let (_, pages) = pages();
    let menu = &pages["pattern-mega-menu"];
    assert_eq!((menu.visible.len(), menu.all.len()), (5, 10));
    let mut s = screens(0.9);
    s.observe(&look(menu, &menu.visible)).unwrap();
    assert!(s.observe(&look(menu, &menu.all)).unwrap().is_new);
}

#[test]
fn the_pattern_pages_have_no_frame_in_common() {
    // Measured: no id is on three of the nine pages, so nothing is page-wide, and leaving page-wide ids out changes nothing here.
    let (_, pages) = pages();
    let visible: Vec<BTreeSet<String>> = pages
        .iter()
        .filter(|(name, _)| name.starts_with("pattern-"))
        .map(|(_, page)| page.visible.clone())
        .collect();
    assert_eq!(visible.len(), 9);
    assert!(chrome_ids(&visible, DEFAULT_CHROME_SHARE).is_empty());
}
