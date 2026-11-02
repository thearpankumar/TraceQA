//! What counts as "the same screen" (backlog EXP-1).
//!
//! The explorer clicks around and records the screens it reaches. To do that it must decide when two looks at a
//! page are one screen: if it is too strict, a ticking clock or a rotating banner makes endless new screens and
//! the walk never ends; if it is too loose, two different pages merge and what is behind the second is never
//! seen. This module is that decision and nothing else: plain functions over plain data, with no browser, no file
//! and no clock.
//!
//! A look at a page is its address, the ids of the components that are visible, the dialog that is open (if any)
//! and the frames that are attached. Two looks are the same screen when
//!
//! * their addresses are equal after cleaning (no `#fragment`, no parameters that change on every visit, the
//!   rest sorted, secrets hidden),
//! * the same dialog is open (or none), and
//! * at least `similarity` (90% unless told otherwise) of their component ids are the same, counted as the Jaccard
//!   index: ids in both, divided by ids in either.
//!
//! The frames are kept but not compared, as the backlog says. The ids that are on most pages (the header and the
//! footer) are left out of the comparison, because a big shared header would make two different views of one address
//! look alike. They are given by the caller, who knows the pages: see [`chrome_ids`].
//!
//! Choices that make the answer repeatable and explainable:
//!
//! * No floating point is used to decide. The similarity is kept in thousandths and compared with whole numbers, so
//!   "exactly 90%" cannot flip with rounding.
//! * Ordered sets are used everywhere and screens are numbered `s0`, `s1`, ... in the order they were first seen,
//!   so the same looks in the same order always give the same screens.
//! * A look is compared with the screens of the same address and dialog only, and joins the one it is most alike
//!   (the earliest of equals), not the first one that is alike enough.
//! * A screen keeps the ids of its **first** look. Were it to follow later looks, A could drift toward B and B toward
//!   C, until one screen matches looks that are nothing like where it began.
//! * Anything that cannot be judged is an error and never a guess: an address that cannot be read, is not http or
//!   https, or is very long, and a look with no component at all (the backlog allows no screen with an empty id set).
//!
//! Known limits. They are written down so that nobody trusts the rule blindly; they are for the walk (EXP-2) to
//! measure on real sites and for the settings below to be tuned against.
//!
//! * **Small pages.** The rule is a share, so on a page of five components one change is a third of it and reads as
//!   a new screen.
//! * **Addresses with an id in them.** `/product/123` and `/product/124` are two addresses, so two screens, however
//!   alike. The walk's `--max-states` budget is what stops a site with thousands of products.
//! * **Growing pages.** A list that loads more rows later can drop below the share; the walk scrolls and settles
//!   before it looks, which mostly avoids that.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use thiserror::Error;
use url::Url;

use crate::url_safety::redact;

/// How alike two looks must be, unless told otherwise: 90% of their ids.
pub const DEFAULT_SIMILARITY: Similarity = Similarity(900);

/// The share of pages an id must be on to count as part of the page's frame (the header, the footer), unless told
/// otherwise.
pub const DEFAULT_CHROME_SHARE: Similarity = Similarity(800);

/// Fewer pages than this give no page-wide ids: with two pages, everything they have in common would be "chrome".
pub const MIN_PAGES_FOR_CHROME: usize = 3;

/// The longest address that is looked at. A longer one is refused, never cut: a cut address can be the address of
/// another page.
pub const MAX_URL_LEN: usize = 4096;

/// Query parameters that change on every visit and say nothing about the screen. A trailing `*` stands for any
/// ending. Names are compared in lower case.
pub const VOLATILE_PARAMS: &[&str] = &["utm_*", "_", "ts", "cache", "fbclid", "gclid"];

/// Why a look cannot be judged.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum StateError {
    /// The text is not an address. The address itself is left out of the message: it may hold a secret.
    #[error("the address could not be read")]
    BadUrl,
    /// Only `http` and `https` addresses are screens.
    #[error("the address is not an http or https address")]
    NotHttp,
    #[error("the address is {0} characters long, over the limit of {MAX_URL_LEN}")]
    TooLong(usize),
    /// A screen with no component cannot be told from any other.
    #[error("the look has no component")]
    EmptyIds,
    #[error("a similarity is a number from 0 to 1, got {0}")]
    BadSimilarity(f64),
}

/// A share from 0 to 1, kept in thousandths so that it is compared with whole numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Similarity(u16);

impl Similarity {
    /// `0.9` is 900 thousandths. A number outside 0 to 1, and NaN, are refused.
    pub fn new(share: f64) -> Result<Self, StateError> {
        if !(0.0..=1.0).contains(&share) {
            return Err(StateError::BadSimilarity(share));
        }
        // 0..=1000 by the check above.
        Ok(Self((share * 1000.0).round() as u16))
    }

    /// The share in thousandths.
    pub fn per_mille(self) -> u16 {
        self.0
    }

    /// Whether `part` out of `whole` is at least this share. Whole numbers only.
    fn holds(self, part: usize, whole: usize) -> bool {
        part as u64 * 1000 >= whole as u64 * u64::from(self.0)
    }
}

/// The name of a screen: `s0` for the first one seen, then `s1`, `s2` and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScreenId(u32);

impl ScreenId {
    /// Where the screen is in the order they were first seen.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for ScreenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s{}", self.0)
    }
}

/// One look at a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    /// The address the browser is at, as it reports it.
    pub url: String,
    /// The ids of the components that are visible.
    pub ids: BTreeSet<String>,
    /// The id of the open dialog (`role=dialog` or `aria-modal`), if any.
    pub modal: Option<String>,
    /// The ids of the frames that are attached. Kept, not compared.
    pub frames: BTreeSet<String>,
}

impl Look {
    pub fn new<I, S>(url: &str, ids: I, modal: Option<&str>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            url: url.to_string(),
            ids: ids.into_iter().map(Into::into).collect(),
            modal: modal.map(str::to_string),
            frames: BTreeSet::new(),
        }
    }
}

/// A screen as the first look at it described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub id: ScreenId,
    /// The cleaned address.
    pub url: String,
    pub modal: Option<String>,
    /// The ids compared: the visible ones without the page-wide ones, or all of them when that leaves none.
    pub ids: BTreeSet<String>,
    pub frames: BTreeSet<String>,
    /// How many looks have been taken to be this screen, the first one included.
    pub looks: u64,
}

/// The answer to a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub id: ScreenId,
    /// True when no known screen was alike and this look made a new one.
    pub is_new: bool,
}

/// The screens seen so far.
#[derive(Debug, Clone)]
pub struct Screens {
    similarity: Similarity,
    chrome: BTreeSet<String>,
    screens: Vec<Screen>,
    /// The screens of each address and dialog, as positions in `screens`, in the order they were seen.
    by_place: HashMap<(String, Option<String>), Vec<usize>>,
}

impl Screens {
    /// `chrome` is the set of page-wide ids to leave out (see [`chrome_ids`]); it may be empty.
    pub fn new(similarity: Similarity, chrome: BTreeSet<String>) -> Self {
        Self {
            similarity,
            chrome,
            screens: Vec::new(),
            by_place: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.screens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.screens.is_empty()
    }

    /// Every screen, in the order they were first seen.
    pub fn all(&self) -> &[Screen] {
        &self.screens
    }

    pub fn get(&self, id: ScreenId) -> Option<&Screen> {
        self.screens.get(id.index())
    }

    /// Takes a look and says which screen it is. The look joins the screen it is most alike, the earliest of equals,
    /// or becomes a new one.
    pub fn observe(&mut self, look: &Look) -> Result<Seen, StateError> {
        if look.ids.is_empty() {
            return Err(StateError::EmptyIds);
        }
        let url = clean_url(&look.url)?;
        let ids = self.compared_ids(&look.ids);
        let place = (url, look.modal.clone());

        let mut best: Option<(usize, usize, usize)> = None; // position, ids in both, ids in either
        for &position in self.by_place.get(&place).map_or(&[][..], Vec::as_slice) {
            let Some((both, either)) = self.alike(&self.screens[position].ids, &ids) else {
                continue;
            };
            // Strictly better only, so the earliest of equals stays: both / either > best_both / best_either.
            let better = best.is_none_or(|(_, b, e)| both * e > b * either);
            if better {
                best = Some((position, both, either));
            }
        }
        if let Some((position, _, _)) = best {
            self.screens[position].looks += 1;
            return Ok(Seen {
                id: self.screens[position].id,
                is_new: false,
            });
        }

        let id = ScreenId(self.screens.len() as u32);
        let position = self.screens.len();
        self.screens.push(Screen {
            id,
            url: place.0.clone(),
            modal: place.1.clone(),
            ids,
            frames: look.frames.clone(),
            looks: 1,
        });
        self.by_place.entry(place).or_default().push(position);
        Ok(Seen { id, is_new: true })
    }

    /// Whether `look` is the screen `id`, by the same rule `observe` uses: the same cleaned address and dialog, and
    /// at least the share of the same ids. It does not ask whether another screen is closer. Use it to find out
    /// whether a page that was opened again is the screen that was wanted: two screens at one address that are not
    /// alike can both be within the share of one look, and `observe` names only the closer.
    pub fn matches(&self, id: ScreenId, look: &Look) -> bool {
        let Some(screen) = self.get(id) else {
            return false;
        };
        if look.ids.is_empty() || look.modal != screen.modal {
            return false;
        }
        let Ok(url) = clean_url(&look.url) else {
            return false;
        };
        url == screen.url && self.alike(&screen.ids, &self.compared_ids(&look.ids)).is_some()
    }

    /// How many ids two sets share and hold between them, when that is at least the share. The cheap check first: the
    /// most the two can share is the smaller set, out of at least the larger, and when even that is not enough there
    /// is nothing to count.
    fn alike(&self, known: &BTreeSet<String>, ids: &BTreeSet<String>) -> Option<(usize, usize)> {
        if !self
            .similarity
            .holds(known.len().min(ids.len()), known.len().max(ids.len()))
        {
            return None;
        }
        let (both, either) = overlap(known, ids);
        self.similarity.holds(both, either).then_some((both, either))
    }

    /// The ids a look is judged by: those that are not page-wide, or all of them when that would leave none.
    fn compared_ids(&self, all: &BTreeSet<String>) -> BTreeSet<String> {
        let own: BTreeSet<String> = all.difference(&self.chrome).cloned().collect();
        if own.is_empty() {
            all.clone()
        } else {
            own
        }
    }
}

/// The ids that are on at least `share` of `pages`, which belong to the frame of the site (the header, the
/// footer) and not to any one screen. With fewer than [`MIN_PAGES_FOR_CHROME`] pages there is nothing to say, and
/// the answer is empty.
pub fn chrome_ids(pages: &[BTreeSet<String>], share: Similarity) -> BTreeSet<String> {
    if pages.len() < MIN_PAGES_FOR_CHROME {
        return BTreeSet::new();
    }
    let mut on: BTreeMap<&str, usize> = BTreeMap::new();
    for page in pages {
        for id in page {
            *on.entry(id.as_str()).or_default() += 1;
        }
    }
    on.into_iter()
        .filter(|(_, pages_with)| share.holds(*pages_with, pages.len()))
        .map(|(id, _)| id.to_string())
        .collect()
}

/// How many ids two sets share and how many they hold between them, in one walk over both.
fn overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> (usize, usize) {
    let (mut a_iter, mut b_iter) = (a.iter().peekable(), b.iter().peekable());
    let (mut both, mut either) = (0, 0);
    while let (Some(x), Some(y)) = (a_iter.peek(), b_iter.peek()) {
        either += 1;
        match x.cmp(y) {
            std::cmp::Ordering::Less => {
                a_iter.next();
            }
            std::cmp::Ordering::Greater => {
                b_iter.next();
            }
            std::cmp::Ordering::Equal => {
                both += 1;
                a_iter.next();
                b_iter.next();
            }
        }
    }
    (both, either + a_iter.count() + b_iter.count())
}

/// The address of a screen: secrets hidden, no `#fragment`, no parameter that changes on every visit, the rest
/// sorted, and the parts a browser treats as equal written one way (host in lower case, no default port).
pub fn clean_url(raw: &str) -> Result<String, StateError> {
    if raw.len() > MAX_URL_LEN {
        return Err(StateError::TooLong(raw.len()));
    }
    // Hidden first, so that two visits that differ only in a token are one address.
    let mut url = Url::parse(&redact(raw)).map_err(|_| StateError::BadUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(StateError::NotHttp);
    }
    url.set_fragment(None);
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_volatile(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        pairs.sort();
        url.query_pairs_mut().clear().extend_pairs(&pairs);
    }
    Ok(url.into())
}

fn is_volatile(name: &str) -> bool {
    let name = name.to_lowercase();
    VOLATILE_PARAMS.iter().any(|listed| match listed.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *listed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "http://127.0.0.1:4173/";

    fn ids(range: std::ops::Range<u32>) -> BTreeSet<String> {
        range.map(|n| format!("c_{n:012}")).collect()
    }

    fn look(url: &str, range: std::ops::Range<u32>) -> Look {
        Look {
            url: url.to_string(),
            ids: ids(range),
            modal: None,
            frames: BTreeSet::new(),
        }
    }

    fn screens() -> Screens {
        Screens::new(DEFAULT_SIMILARITY, BTreeSet::new())
    }

    // ---- is this look that screen? -----------------------------------------------------------

    #[test]
    fn a_look_matches_the_screen_it_made_and_one_a_little_different_from_it() {
        let mut known = screens();
        let seen = known.observe(&look(HOME, 0..20)).unwrap();
        assert!(known.matches(seen.id, &look(HOME, 0..20)));
        // 19 of 21 is 90.5%: the same screen.
        assert!(known.matches(seen.id, &look(HOME, 1..21)));
        // 18 of 22 is 81.8%.
        assert!(!known.matches(seen.id, &look(HOME, 2..22)));
    }

    #[test]
    fn a_look_at_another_address_or_with_another_dialog_is_not_the_screen() {
        let mut known = screens();
        let seen = known.observe(&look(HOME, 0..20)).unwrap();
        assert!(!known.matches(seen.id, &look("http://127.0.0.1:4173/cart", 0..20)));
        let mut with_dialog = look(HOME, 0..20);
        with_dialog.modal = Some("dialog \"Help\"".into());
        assert!(!known.matches(seen.id, &with_dialog));
        // And a look with nothing on it is not a screen at all.
        assert!(!known.matches(seen.id, &look(HOME, 0..0)));
    }

    #[test]
    fn a_look_is_the_screen_it_is_alike_even_when_another_is_closer() {
        // Two screens at one address that are not alike (18 of 24 is 75%). A look between them is within 90% of the
        // first and closer to the second: `observe` says the second, and it is still the first by the rule.
        let mut known = screens();
        let first = known.observe(&look(HOME, 0..20)).unwrap().id;
        let second = known.observe(&look(HOME, 0..24)).unwrap().id;
        assert_ne!(first, second);
        let between = look(HOME, 0..22);
        assert_eq!(known.clone().observe(&between).unwrap().id, second);
        assert!(known.matches(first, &between));
        assert!(known.matches(second, &between));
    }

    #[test]
    fn page_wide_ids_do_not_count_when_a_look_is_matched() {
        let chrome: BTreeSet<String> = ids(100..110);
        let mut known = Screens::new(DEFAULT_SIMILARITY, chrome.clone());
        let mut base = look(HOME, 0..20);
        base.ids.extend(chrome.clone());
        let seen = known.observe(&base).unwrap();
        // The header is gone; the page's own 20 are all there.
        assert!(known.matches(seen.id, &look(HOME, 0..20)));
    }

    #[test]
    fn a_screen_that_does_not_exist_matches_nothing() {
        let known = screens();
        let other = {
            let mut other = screens();
            other.observe(&look(HOME, 0..20)).unwrap().id
        };
        assert!(!known.matches(other, &look(HOME, 0..20)));
    }

    // ---- the share ---------------------------------------------------------------------------

    #[test]
    fn a_share_is_kept_in_thousandths_and_a_bad_one_is_refused() {
        assert_eq!(Similarity::new(0.9).unwrap().per_mille(), 900);
        assert_eq!(Similarity::new(1.0).unwrap().per_mille(), 1000);
        assert_eq!(Similarity::new(0.0).unwrap().per_mille(), 0);
        assert_eq!(Similarity::new(0.8999).unwrap().per_mille(), 900);
        for bad in [-0.1, 1.1, f64::NAN, f64::INFINITY, -f64::INFINITY] {
            assert!(
                matches!(Similarity::new(bad), Err(StateError::BadSimilarity(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_share_holds_exactly_at_its_edge_and_not_a_hair_below() {
        let nine_tenths = Similarity::new(0.9).unwrap();
        assert!(nine_tenths.holds(9, 10));
        assert!(!nine_tenths.holds(8, 10));
        assert!(nine_tenths.holds(19, 21), "19 of 21 is 90.5%");
        assert!(!nine_tenths.holds(17, 23));
        let whole = Similarity::new(1.0).unwrap();
        assert!(whole.holds(5, 5));
        assert!(!whole.holds(4, 5));
        // Nothing is asked of a share of 0, and 0 of 0 does not fail it.
        assert!(Similarity::new(0.0).unwrap().holds(0, 7));
        assert!(nine_tenths.holds(0, 0));
    }

    // ---- the rule of the backlog -------------------------------------------------------------

    #[test]
    fn twenty_ids_that_differ_in_one_are_the_same_screen_and_in_three_are_not() {
        let mut s = screens();
        let first = s.observe(&look(HOME, 0..20)).unwrap();
        assert_eq!((first.id.to_string().as_str(), first.is_new), ("s0", true));
        // One id swapped for another: 19 shared out of 21.
        let one = s.observe(&look(HOME, 1..21)).unwrap();
        assert_eq!((one.id, one.is_new), (first.id, false));
        // Three swapped: 17 shared out of 23.
        let three = s.observe(&look(HOME, 3..23)).unwrap();
        assert!(three.is_new);
        assert_eq!(three.id.to_string(), "s1");
    }

    #[test]
    fn a_share_of_one_asks_for_the_same_ids() {
        let mut s = Screens::new(Similarity::new(1.0).unwrap(), BTreeSet::new());
        let a = s.observe(&look(HOME, 0..20)).unwrap();
        assert!(!s.observe(&look(HOME, 0..20)).unwrap().is_new);
        // One more id is no longer the same screen.
        assert!(s.observe(&look(HOME, 0..21)).unwrap().is_new);
        assert_eq!(a.id.index(), 0);
    }

    #[test]
    fn a_look_twice_is_one_screen_and_counts_two_looks() {
        let mut s = screens();
        assert!(s.is_empty());
        let a = s.observe(&look(HOME, 0..10)).unwrap();
        let b = s.observe(&look(HOME, 0..10)).unwrap();
        assert_eq!((a.id, b.is_new), (b.id, false));
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
        assert_eq!(s.get(a.id).unwrap().looks, 2);
    }

    #[test]
    fn the_order_of_the_ids_does_not_matter() {
        let forward = Look::new(HOME, ["c_a", "c_b", "c_c"], None);
        let backward = Look::new(HOME, ["c_c", "c_b", "c_a"], None);
        assert_eq!(forward, backward);
        let mut s = screens();
        s.observe(&forward).unwrap();
        assert!(!s.observe(&backward).unwrap().is_new);
    }

    #[test]
    fn the_same_ids_at_another_address_are_another_screen() {
        let mut s = screens();
        s.observe(&look("http://127.0.0.1:4173/products", 0..10)).unwrap();
        assert!(s.observe(&look("http://127.0.0.1:4173/cart", 0..10)).unwrap().is_new);
    }

    #[test]
    fn the_same_ids_with_another_dialog_open_are_another_screen() {
        let mut s = screens();
        let plain = look(HOME, 0..10);
        let with_dialog = Look {
            modal: Some("c_dialog".into()),
            ..look(HOME, 0..10)
        };
        let other_dialog = Look {
            modal: Some("c_other".into()),
            ..look(HOME, 0..10)
        };
        s.observe(&plain).unwrap();
        assert!(s.observe(&with_dialog).unwrap().is_new);
        assert!(s.observe(&other_dialog).unwrap().is_new);
        // And the same dialog again is the screen it was.
        assert!(!s.observe(&with_dialog).unwrap().is_new);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn screens_are_named_in_the_order_they_were_first_seen() {
        let mut s = screens();
        let names: Vec<String> = (0..4)
            .map(|n| {
                s.observe(&look(&format!("{HOME}page{n}"), 0..5))
                    .unwrap()
                    .id
                    .to_string()
            })
            .collect();
        assert_eq!(names, ["s0", "s1", "s2", "s3"]);
        assert_eq!(s.all().len(), 4);
        assert_eq!(s.get(ScreenId(2)).unwrap().url, "http://127.0.0.1:4173/page2");
        assert!(s.get(ScreenId(9)).is_none());
    }

    #[test]
    fn the_same_looks_in_the_same_order_give_the_same_screens_every_time() {
        let run = || {
            let mut s = screens();
            let looks = [(0..20), (1..21), (5..25), (0..20), (10..30), (6..26)];
            looks
                .into_iter()
                .map(|r| s.observe(&look(HOME, r)).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    // ---- which screen a look joins ------------------------------------------------------------

    #[test]
    fn a_look_joins_the_screen_it_is_most_alike_and_not_the_first_that_is_alike_enough() {
        // Two screens that are not alike (5 of 15 shared, a third), and a look that is alike enough to both.
        let half = Similarity::new(0.5).unwrap();
        let mut s = Screens::new(half, BTreeSet::new());
        assert_eq!(s.observe(&look(HOME, 1..11)).unwrap().id.index(), 0);
        assert_eq!(s.observe(&look(HOME, 6..16)).unwrap().id.index(), 1);
        // 4..14 shares 7 of 13 with the first (54%) and 8 of 12 with the second (67%).
        let joined = s.observe(&look(HOME, 4..14)).unwrap();
        assert_eq!((joined.id.index(), joined.is_new), (1, false));
    }

    #[test]
    fn of_two_equally_alike_screens_the_earlier_one_is_joined() {
        let third = Similarity::new(0.3).unwrap();
        let mut s = Screens::new(third, BTreeSet::new());
        s.observe(&look(HOME, 1..11)).unwrap();
        s.observe(&look(HOME, 11..21)).unwrap();
        // 6..16 shares 5 of 15 with each.
        assert_eq!(s.observe(&look(HOME, 6..16)).unwrap().id.index(), 0);
    }

    #[test]
    fn a_screen_keeps_the_ids_of_its_first_look() {
        let mut s = screens();
        let first = s.observe(&look(HOME, 0..20)).unwrap();
        s.observe(&look(HOME, 1..21)).unwrap();
        assert_eq!(s.get(first.id).unwrap().ids, ids(0..20));
        // So a look 1..21 drifted to is judged against 0..20 and not against what it grew into: 2..22 shares
        // 18 of 22 with 0..20, which is not enough, even though it is alike 1..21.
        assert!(s.observe(&look(HOME, 2..22)).unwrap().is_new);
    }

    #[test]
    fn the_frames_of_the_first_look_are_kept_and_never_compared() {
        let mut s = screens();
        let with_frame = Look {
            frames: ["main".to_string(), "f1".to_string()].into_iter().collect(),
            ..look(HOME, 0..10)
        };
        let seen = s.observe(&with_frame).unwrap();
        // Another set of frames does not make another screen.
        assert!(!s.observe(&look(HOME, 0..10)).unwrap().is_new);
        assert_eq!(s.get(seen.id).unwrap().frames.len(), 2);
    }

    // ---- refusing what cannot be judged --------------------------------------------------------

    #[test]
    fn a_look_with_no_component_is_refused() {
        let mut s = screens();
        assert_eq!(
            s.observe(&Look::new(HOME, Vec::<String>::new(), None)),
            Err(StateError::EmptyIds)
        );
        assert!(s.is_empty());
    }

    #[test]
    fn an_address_that_cannot_be_judged_is_refused() {
        let mut s = screens();
        assert_eq!(s.observe(&look("not an address", 0..5)), Err(StateError::BadUrl));
        assert_eq!(s.observe(&look("", 0..5)), Err(StateError::BadUrl));
        assert_eq!(s.observe(&look("ftp://files.test/a", 0..5)), Err(StateError::NotHttp));
        assert_eq!(s.observe(&look("file:///etc/passwd", 0..5)), Err(StateError::NotHttp));
        assert_eq!(s.observe(&look("data:text/html,hello", 0..5)), Err(StateError::NotHttp));
        assert_eq!(s.observe(&look("about:blank", 0..5)), Err(StateError::NotHttp));
        let long = format!("http://a.test/{}", "x".repeat(MAX_URL_LEN));
        assert_eq!(s.observe(&look(&long, 0..5)), Err(StateError::TooLong(long.len())));
        assert!(s.is_empty(), "a refused look makes no screen");
    }

    #[test]
    fn an_error_never_shows_the_address() {
        let secret = "http://u:pw@h.test/?token=SECRET";
        let message = StateError::BadUrl.to_string();
        assert!(!message.contains("SECRET") && !message.contains(secret));
        // An address at the very limit is accepted, one over is not.
        let at_limit = format!("http://a.test/{}", "x".repeat(MAX_URL_LEN - "http://a.test/".len()));
        assert_eq!(at_limit.len(), MAX_URL_LEN);
        assert!(clean_url(&at_limit).is_ok());
        assert_eq!(
            clean_url(&format!("{at_limit}x")),
            Err(StateError::TooLong(MAX_URL_LEN + 1))
        );
    }

    // ---- the address --------------------------------------------------------------------------

    #[test]
    fn the_fragment_is_dropped() {
        assert_eq!(clean_url("http://a.test/p#top").unwrap(), "http://a.test/p");
        assert_eq!(clean_url("http://a.test/p?x=1#top").unwrap(), "http://a.test/p?x=1");
    }

    #[test]
    fn parameters_that_change_on_every_visit_are_dropped_and_the_rest_kept() {
        assert_eq!(
            clean_url("http://a.test/p?utm_source=a&utm_medium=b&page=2&_=123&ts=9&cache=no&fbclid=x&gclid=y").unwrap(),
            "http://a.test/p?page=2"
        );
        assert_eq!(
            clean_url("http://a.test/p?UTM_Source=a&TS=1").unwrap(),
            "http://a.test/p"
        );
        // A name that only starts like a listed one is not dropped, and `utm` alone is not `utm_*`.
        assert_eq!(
            clean_url("http://a.test/p?timestamp=1&utm=1&cached=1").unwrap(),
            "http://a.test/p?cached=1&timestamp=1&utm=1"
        );
    }

    #[test]
    fn the_parameters_that_stay_are_sorted_so_their_order_is_not_a_difference() {
        assert_eq!(
            clean_url("http://a.test/p?b=2&a=1").unwrap(),
            clean_url("http://a.test/p?a=1&b=2").unwrap()
        );
        assert_eq!(clean_url("http://a.test/p?a=2&a=1").unwrap(), "http://a.test/p?a=1&a=2");
        assert_eq!(clean_url("http://a.test/p?").unwrap(), "http://a.test/p");
    }

    #[test]
    fn what_a_browser_treats_as_equal_is_written_one_way() {
        let canonical = "http://a.test/p";
        for raw in [
            "HTTP://A.TEST/p",
            "http://a.test:80/p",
            "http://a.test/./p",
            "http://a.test/x/../p",
        ] {
            assert_eq!(clean_url(raw).unwrap(), canonical, "{raw}");
        }
        assert_eq!(clean_url("https://a.test:443/p").unwrap(), "https://a.test/p");
        assert_eq!(clean_url("http://a.test:8080/p").unwrap(), "http://a.test:8080/p");
        // A path is kept as it is: `/p/` and `/p` can be two pages.
        assert_ne!(
            clean_url("http://a.test/p/").unwrap(),
            clean_url("http://a.test/p").unwrap()
        );
    }

    #[test]
    fn secrets_in_an_address_are_hidden_and_two_visits_that_differ_only_in_one_are_one_address() {
        let one = clean_url("http://u:pw@a.test/p?token=ONE&page=2").unwrap();
        let two = clean_url("http://a.test/p?page=2&token=TWO").unwrap();
        assert_eq!(one, two);
        assert!(
            one.contains("REDACTED") && !one.contains("ONE") && !one.contains("pw@"),
            "{one}"
        );
    }

    // ---- the page-wide ids --------------------------------------------------------------------

    #[test]
    fn a_big_shared_header_does_not_make_two_views_of_one_address_alike() {
        // 40 ids of the frame of the site and 2 of each view: 40 of 44 shared is 91%.
        let frame = ids(0..40);
        let view = |own: std::ops::Range<u32>| Look {
            ids: frame.union(&ids(own)).cloned().collect(),
            ..look(HOME, 0..1)
        };
        let mut without = screens();
        without.observe(&view(100..102)).unwrap();
        assert!(
            !without.observe(&view(200..202)).unwrap().is_new,
            "merged: the header drowns the views"
        );

        let mut with = Screens::new(DEFAULT_SIMILARITY, frame.clone());
        with.observe(&view(100..102)).unwrap();
        assert!(with.observe(&view(200..202)).unwrap().is_new);
        // The screen holds only what is its own.
        assert_eq!(with.all()[0].ids, ids(100..102));
    }

    #[test]
    fn a_look_made_only_of_page_wide_ids_keeps_them_so_that_no_screen_is_empty() {
        let frame = ids(0..5);
        let mut s = Screens::new(DEFAULT_SIMILARITY, frame.clone());
        let only_frame = Look {
            ids: frame.clone(),
            ..look("http://127.0.0.1:4173/error", 0..1)
        };
        let seen = s.observe(&only_frame).unwrap();
        assert_eq!(s.get(seen.id).unwrap().ids, frame);
        assert!(!s.observe(&only_frame).unwrap().is_new);
    }

    #[test]
    fn the_page_wide_ids_are_those_on_enough_of_the_pages() {
        let pages: Vec<BTreeSet<String>> = vec![
            ids(0..10), // the frame is 0..4
            ids(0..4).union(&ids(10..20)).cloned().collect(),
            ids(0..4).union(&ids(20..30)).cloned().collect(),
            ids(0..4).union(&ids(30..40)).cloned().collect(),
            ids(0..3).union(&ids(40..50)).cloned().collect(),
        ];
        // 0, 1 and 2 are on five of five pages, 3 on four of five (80%): both count at 80%.
        assert_eq!(chrome_ids(&pages, DEFAULT_CHROME_SHARE), ids(0..4));
        // At 100%, only what is on every page.
        assert_eq!(chrome_ids(&pages, Similarity::new(1.0).unwrap()), ids(0..3));
        // An id on three of five pages is not at 80%.
        let three_of_five: Vec<BTreeSet<String>> = vec![ids(0..1), ids(0..1), ids(0..1), ids(5..6), ids(6..7)];
        assert!(chrome_ids(&three_of_five, DEFAULT_CHROME_SHARE).is_empty());
        assert_eq!(chrome_ids(&three_of_five, Similarity::new(0.6).unwrap()), ids(0..1));
    }

    #[test]
    fn fewer_than_three_pages_say_nothing_about_a_frame() {
        let same = ids(0..10);
        assert!(chrome_ids(&[], DEFAULT_CHROME_SHARE).is_empty());
        assert!(chrome_ids(std::slice::from_ref(&same), DEFAULT_CHROME_SHARE).is_empty());
        assert!(chrome_ids(&[same.clone(), same.clone()], DEFAULT_CHROME_SHARE).is_empty());
        assert_eq!(
            chrome_ids(&[same.clone(), same.clone(), same.clone()], DEFAULT_CHROME_SHARE),
            same
        );
    }

    // ---- counting overlap, and the shortcut that skips it ------------------------------------

    #[test]
    fn overlap_counts_what_two_sets_share_and_what_they_hold_between_them() {
        assert_eq!(overlap(&ids(0..20), &ids(1..21)), (19, 21));
        assert_eq!(overlap(&ids(0..5), &ids(5..10)), (0, 10));
        assert_eq!(overlap(&ids(0..5), &ids(0..5)), (5, 5));
        assert_eq!(overlap(&ids(0..5), &BTreeSet::new()), (0, 5));
        assert_eq!(overlap(&BTreeSet::new(), &ids(0..7)), (0, 7));
        assert_eq!(overlap(&BTreeSet::new(), &BTreeSet::new()), (0, 0));
        // One set is inside the other.
        assert_eq!(overlap(&ids(2..4), &ids(0..10)), (2, 10));
    }

    /// A small pseudo-random generator, so that the tests below need no library and give the same numbers every run.
    struct Dice(u64);
    impl Dice {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn set(&mut self, universe: u64, size: u64) -> BTreeSet<String> {
            (0..size).map(|_| format!("c_{:012}", self.next() % universe)).collect()
        }
    }

    #[test]
    fn the_shortcut_by_size_never_skips_a_pair_that_is_alike_enough() {
        let mut dice = Dice(0x9E37_79B9_7F4A_7C15);
        for share in [0.5, 0.75, 0.9, 1.0] {
            let similarity = Similarity::new(share).unwrap();
            for _ in 0..400 {
                let (universe, a_size, b_size) = (10 + dice.next() % 60, 1 + dice.next() % 40, 1 + dice.next() % 40);
                let (a, b) = (dice.set(universe, a_size), dice.set(universe, b_size));
                let (both, either) = overlap(&a, &b);
                if similarity.holds(both, either) {
                    assert!(
                        similarity.holds(a.len().min(b.len()), a.len().max(b.len())),
                        "skipped a pair that matches: {} and {} ids, {both} of {either} shared, share {share}",
                        a.len(),
                        b.len()
                    );
                }
            }
        }
    }

    #[test]
    fn a_screen_is_always_the_same_as_itself_and_two_looks_agree_whichever_is_first() {
        let mut dice = Dice(0x2545_F491_4F6C_DD1D);
        for _ in 0..300 {
            let (universe, a_size, b_size) = (5 + dice.next() % 50, 1 + dice.next() % 30, 1 + dice.next() % 30);
            let (a, b) = (dice.set(universe, a_size), dice.set(universe, b_size));
            let make = |ids: &BTreeSet<String>| Look {
                ids: ids.clone(),
                ..look(HOME, 0..1)
            };
            // Itself.
            let mut s = screens();
            s.observe(&make(&a)).unwrap();
            assert!(!s.observe(&make(&a)).unwrap().is_new);
            // Whichever comes first, both are one screen or both are two.
            let (mut ab, mut ba) = (screens(), screens());
            ab.observe(&make(&a)).unwrap();
            ba.observe(&make(&b)).unwrap();
            assert_eq!(
                ab.observe(&make(&b)).unwrap().is_new,
                ba.observe(&make(&a)).unwrap().is_new
            );
            assert_eq!(overlap(&a, &b), overlap(&b, &a));
        }
    }
}
