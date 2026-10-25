//! Judging what was read: plain data in, a warning or marks out. Nothing here talks to a driver.

use std::collections::HashMap;
use std::time::Duration;

use traceqa_common::types::{Component, PageGotoResult, Warning};

use super::frames::{element_key, ElementKey, Reading};
use super::warning;
use crate::url_safety::redact;

/// Address words that mean a sign-in page.
const SIGN_IN_WORDS: [&str; 6] = ["login", "log-in", "signin", "sign-in", "sign_in", "auth"];

/// A run given cookies and stored values that still lands on what looks like a sign-in page
/// was not let in: the session has expired, or the state belongs to another site. Without this
/// the run would quietly read the sign-in page in place of the page asked for.
fn sign_in_warning(reading: &Reading, final_url: &str) -> Option<Warning> {
    let path = final_url
        .split_once("://")
        .map_or(final_url, |(_, rest)| rest)
        .to_lowercase();
    let path = path.split_once('/').map_or("", |(_, path)| path);
    let address = SIGN_IN_WORDS.iter().any(|word| path.contains(word));
    let password = reading.elements.iter().any(|located| {
        located
            .element
            .attrs
            .get("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
    });
    let sign = match (address, password) {
        (true, _) => "its address",
        (false, true) => "its password field",
        (false, false) => return None,
    };
    Some(warning(
        "session_not_accepted",
        format!(
            "The browser started with the stored session, but by {sign} the page is a sign-in page, so the session may have expired. Sign in again to make a new state file."
        ),
    ))
}

/// The least time between the two looks at the page that tell which names change. A countdown
/// ticks every second, so a second is enough to see it.
pub(super) const NAME_WATCH: Duration = Duration::from_millis(1000);

/// Role, name and position of every element, by element.
pub(super) type Names = HashMap<ElementKey, (String, String, Option<(f64, f64)>)>;

pub(super) fn names_of(reading: &Reading) -> Names {
    reading
        .elements
        .iter()
        .map(|located| {
            let at = located.page_box.as_ref().map(|b| (b.x, b.y));
            let key = element_key(&located.frame_id, &located.element.selectors);
            (key, (located.element.role.clone(), located.element.name.clone(), at))
        })
        .collect()
}

/// Marks the elements whose name is not the one they had before the walk. Only the same
/// element counts: same place in the page, same role, same position. Content that loaded on
/// the way and moved others down is new, not renamed.
pub(super) fn mark_changed_names(reading: &mut Reading, before: &Names) {
    for located in &mut reading.elements {
        let key = element_key(&located.frame_id, &located.element.selectors);
        let Some((role, name, at)) = before.get(&key) else {
            continue;
        };
        let now = located.page_box.as_ref().map(|b| (b.x, b.y));
        let same_place = match (at, now) {
            (Some(a), Some(b)) => (a.0 - b.0).abs() <= 2.0 && (a.1 - b.1).abs() <= 2.0,
            _ => false,
        };
        if same_place && *role == located.element.role && *name != located.element.name {
            located.name_changes = true;
        }
    }
}

/// Watches which controls the page holds while it is scrolled. A long table or feed that keeps
/// only the rows near the screen holds about as many at any moment, but not the same ones:
/// rows it showed earlier are gone, and so they are gone when the page is read.
#[derive(Default)]
pub(super) struct Removals {
    seen: std::collections::HashSet<String>,
    most: u64,
    dropped: u64,
}

impl Removals {
    pub(super) fn saw(&mut self, keys: Vec<String>) {
        let present: std::collections::HashSet<String> = keys.into_iter().collect();
        self.most = self.most.max(present.len() as u64);
        self.seen.extend(present.iter().cloned());
        self.dropped = self.dropped.max(self.seen.difference(&present).count() as u64);
    }

    /// A few controls come and go on any page, a menu closing for one. Only a real drop counts.
    pub(super) fn warning(&self) -> Option<Warning> {
        (self.dropped >= 5 && self.dropped * 10 >= self.most).then(|| {
            warning(
                "elements_removed_while_scrolling",
                format!(
                    "The page removed {} controls it had shown while it was scrolled, as long lists that keep only the rows near the screen do. Rows that had left the screen are not listed.",
                    self.dropped
                ),
            )
        })
    }
}

/// One warning for every component a click would not reach, because something lies on top of it:
/// a cookie banner or a modal's backdrop often covers a whole page at once.
pub(super) fn covered_warning(components: &[Component]) -> Option<Warning> {
    let covered: Vec<(&Component, &str)> = components
        .iter()
        .filter_map(|component| {
            let instance = component.instances.iter().find(|i| i.state.covered == Some(true))?;
            Some((
                component,
                instance.state.covered_by.as_deref().unwrap_or("another element"),
            ))
        })
        .collect();
    let (first, by) = covered.first()?;
    let name = if first.name.is_empty() {
        "(no name)"
    } else {
        &first.name
    };
    let more = match covered.len() {
        1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    Some(warning(
        "covered_elements",
        format!(
            "{} {name:?} is under {by}{more}, so a click would not reach {}.",
            first.role,
            if covered.len() == 1 { "it" } else { "them" }
        ),
    ))
}

/// One warning when requests for scripts, styles, data or frames failed while the page loaded: the
/// page may then be missing parts, and so may the list. Only drivers that can see the network report it.
pub(super) fn requests_failed_warning(reading: &Reading) -> Option<Warning> {
    let failed = reading.requests_failed.as_ref().filter(|failed| failed.count > 0)?;
    // The driver already removed query strings. Redacting again keeps a secret in a path from leaking.
    let urls: Vec<String> = failed.urls.iter().map(|url| redact(url)).collect();
    let first = if urls.is_empty() {
        String::new()
    } else {
        format!(" (first: {})", urls.join(", "))
    };
    Some(warning(
        "requests_failed",
        format!(
            "{} request(s) for scripts, styles or data failed while the page loaded{first}, so parts of the page may be missing from this list.",
            failed.count
        ),
    ))
}

/// What the answer to the page request already says: an error status, closed shadow roots, a
/// page that had not finished loading.
pub(super) fn landing_warnings(page: &PageGotoResult) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if let Some(status) = page.status.filter(|status| *status >= 400) {
        warnings.push(warning(
            "http_error_status",
            format!("The page answered with HTTP {status}, so this may be an error page."),
        ));
    }
    if let Some(closed) = page.closed_shadow_roots.filter(|closed| *closed > 0) {
        warnings.push(warning(
            "closed_shadow_roots",
            format!(
                "The page has {closed} closed shadow root{}. No script can read inside one, so what it holds is not listed.",
                if closed == 1 { "" } else { "s" }
            ),
        ));
    }
    if page.load_complete == Some(false) {
        warnings.push(warning(
            "page_load_incomplete",
            "The page's content arrived, but it had not finished loading when time ran out, so it was read as it stood."
                .to_string(),
        ));
    }
    warnings
}

/// Warnings for a run that started with a stored session: a sign-in page means the session was
/// not accepted, and saved pictures of a signed-in page can show personal data.
pub(super) fn stored_session_warnings(reading: &Reading, final_url: &str, saved_pictures: bool) -> Vec<Warning> {
    let mut warnings: Vec<Warning> = sign_in_warning(reading, final_url).into_iter().collect();
    if saved_pictures {
        warnings.push(warning(
            "signed_in_pictures",
            "This run used a stored session and saved pictures (page.png and crops/). They can show personal data. Keep the run folder private and delete it when done.".to_string(),
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::Located;
    use traceqa_common::types::{CollectorResult, ComponentsFile, RequestsFailed};

    /// The shop example's elements, as one reading of a page.
    fn reading() -> Reading {
        let collected: CollectorResult =
            serde_json::from_str(include_str!("../../../../spec/examples/CollectorResult.shop.json")).unwrap();
        Reading {
            elements: collected
                .elements
                .into_iter()
                .map(|element| Located {
                    frame_id: "main".into(),
                    frame_step: None,
                    page_box: Some(element.box_.clone()),
                    name_changes: false,
                    element,
                })
                .collect(),
            title: collected.title,
            scroll: collected.scroll,
            warnings: Vec::new(),
            frames: 1,
            requests_failed: None,
        }
    }

    fn with_password(mut reading: Reading) -> Reading {
        reading.elements[0]
            .element
            .attrs
            .insert("type".into(), "Password".into());
        reading
    }

    fn failed(count: u64, urls: &[&str]) -> Reading {
        let mut reading = reading();
        reading.requests_failed = Some(RequestsFailed {
            count,
            urls: urls.iter().map(ToString::to_string).collect(),
        });
        reading
    }

    #[test]
    fn requests_failed_warning_names_count_and_first_urls() {
        let urls = [
            "https://shop.test/a.js",
            "https://shop.test/b.js?token=SEKRET",
            "https://shop.test/c.css",
            "https://shop.test/api/d",
            "https://shop.test/e.js",
        ];
        let warning = requests_failed_warning(&failed(7, &urls)).unwrap();
        assert_eq!(warning.code, "requests_failed");
        assert!(warning.message.starts_with("7 request(s)"), "{}", warning.message);
        for url in [
            "https://shop.test/a.js",
            "https://shop.test/c.css",
            "https://shop.test/e.js",
        ] {
            assert!(warning.message.contains(url), "{}", warning.message);
        }
        assert!(!warning.message.contains("SEKRET"), "{}", warning.message);
        // The count is shown even when the driver names no address.
        let bare = requests_failed_warning(&failed(2, &[])).unwrap();
        assert!(bare.message.starts_with("2 request(s)") && !bare.message.contains("first:"));
    }

    #[test]
    fn requests_failed_warning_is_absent_for_zero_or_none() {
        assert!(requests_failed_warning(&failed(0, &[])).is_none());
        assert!(requests_failed_warning(&reading()).is_none());
    }

    #[test]
    fn sign_in_warning_fires_for_a_login_redirect_and_not_for_a_normal_page() {
        let page = reading();
        assert!(sign_in_warning(&page, "https://shop.test/").is_none());
        // Only the path counts: a host that happens to contain a word is not a sign-in page.
        assert!(sign_in_warning(&page, "https://auth.shop.test/").is_none());

        let by_address = sign_in_warning(&page, "https://shop.test/Account/Login?next=/cart").unwrap();
        assert_eq!(by_address.code, "session_not_accepted");
        assert!(by_address.message.contains("by its address the page is a sign-in page"));

        let by_field = sign_in_warning(&with_password(reading()), "https://shop.test/account").unwrap();
        assert!(by_field
            .message
            .contains("by its password field the page is a sign-in page"));
        // The address is named first when both say so.
        let both = sign_in_warning(&with_password(reading()), "https://shop.test/sign-in").unwrap();
        assert!(both.message.contains("by its address"));
    }

    #[test]
    fn stored_session_warnings_add_the_pictures_notice_only_when_pictures_were_saved() {
        let page = reading();
        assert!(stored_session_warnings(&page, "https://shop.test/", false).is_empty());
        let codes = |url: &str, pictures| {
            stored_session_warnings(&page, url, pictures)
                .into_iter()
                .map(|w| w.code)
                .collect::<Vec<_>>()
        };
        assert_eq!(codes("https://shop.test/", true), ["signed_in_pictures"]);
        assert_eq!(
            codes("https://shop.test/login", true),
            ["session_not_accepted", "signed_in_pictures"]
        );
    }

    #[test]
    fn landing_warnings_describe_the_way_in() {
        let page = |json: &str| serde_json::from_str::<PageGotoResult>(json).unwrap();
        assert!(landing_warnings(&page(r#"{"finalUrl":"https://a.test/","status":200,"title":""}"#)).is_empty());
        let codes: Vec<String> = landing_warnings(&page(
            r#"{"finalUrl":"https://a.test/","status":503,"title":"","closedShadowRoots":1,"loadComplete":false}"#,
        ))
        .into_iter()
        .map(|w| w.code)
        .collect();
        assert_eq!(
            codes,
            ["http_error_status", "closed_shadow_roots", "page_load_incomplete"]
        );
        let two = landing_warnings(&page(
            r#"{"finalUrl":"https://a.test/","status":200,"title":"","closedShadowRoots":2}"#,
        ));
        assert!(two[0].message.starts_with("The page has 2 closed shadow roots."));
    }

    fn named(mut reading: Reading, name: &str, dx: f64, role: Option<&str>) -> Reading {
        let located = &mut reading.elements[0];
        located.element.name = name.to_string();
        if let Some(role) = role {
            located.element.role = role.to_string();
        }
        if let Some(at) = located.page_box.as_mut() {
            at.x += dx;
        }
        reading
    }

    #[test]
    fn mark_changed_names_marks_same_place_same_role_different_name() {
        let before = names_of(&reading());
        let mut after = named(reading(), "Added", 0.0, None);
        mark_changed_names(&mut after, &before);
        assert!(after.elements[0].name_changes);
        // The others kept their names.
        assert!(after.elements[1..].iter().all(|located| !located.name_changes));

        let mut same = reading();
        mark_changed_names(&mut same, &before);
        assert!(same.elements.iter().all(|located| !located.name_changes));
    }

    #[test]
    fn mark_changed_names_ignores_moved_or_new_elements() {
        let before = names_of(&reading());
        // Two pixels either way is the same place; three is something that moved.
        let mut near = named(reading(), "Added", 2.0, None);
        mark_changed_names(&mut near, &before);
        assert!(near.elements[0].name_changes);
        let mut moved = named(reading(), "Added", 3.0, None);
        mark_changed_names(&mut moved, &before);
        assert!(!moved.elements[0].name_changes);
        // A different role is a different element.
        let mut other = named(reading(), "Added", 0.0, Some("link"));
        mark_changed_names(&mut other, &before);
        assert!(!other.elements[0].name_changes);
        // An element that was not there before is new, not renamed.
        let mut fresh = named(reading(), "Added", 0.0, None);
        fresh.elements[0].element.selectors = vec!["#new".into()];
        mark_changed_names(&mut fresh, &before);
        assert!(!fresh.elements[0].name_changes);
    }

    fn keys(range: std::ops::Range<u32>) -> Vec<String> {
        range.map(|n| format!("li|row {n}")).collect()
    }

    #[test]
    fn removals_report_items_dropped_while_scrolling() {
        // A list that keeps a window of 40 rows and drops the ones that left the screen.
        let mut windowed = Removals::default();
        windowed.saw(keys(0..40));
        windowed.saw(keys(10..50));
        assert_eq!((windowed.most, windowed.dropped), (40, 10));
        let warning = windowed.warning().unwrap();
        assert_eq!(warning.code, "elements_removed_while_scrolling");
        assert!(warning.message.starts_with("The page removed 10 controls"));

        // A page that only grows drops nothing.
        let mut growing = Removals::default();
        growing.saw(keys(0..40));
        growing.saw(keys(0..80));
        assert!(growing.warning().is_none());
    }

    #[test]
    fn removals_ignore_a_few_controls_coming_and_going() {
        let mut few = Removals::default();
        few.saw(keys(0..40));
        few.saw(keys(4..44));
        assert_eq!(few.dropped, 4);
        assert!(few.warning().is_none());

        // Five gone, but out of a hundred is under a tenth.
        let mut small_share = Removals::default();
        small_share.saw(keys(0..100));
        small_share.saw(keys(5..105));
        assert_eq!(small_share.dropped, 5);
        assert!(small_share.warning().is_none());
    }

    /// The shop example's component, under a cookie banner or not, renamed.
    fn component(name: &str, covered: Option<bool>) -> Component {
        let file: ComponentsFile =
            serde_json::from_str(include_str!("../../../../spec/examples/ComponentsFile.shop.json")).unwrap();
        let mut component = file.components.into_iter().next().unwrap();
        component.name = name.to_string();
        for instance in &mut component.instances {
            instance.state.covered = covered;
            instance.state.covered_by = covered.filter(|c| *c).map(|_| "#cookie-banner".to_string());
        }
        component
    }

    #[test]
    fn covered_warning_none_when_nothing_covered() {
        assert!(covered_warning(&[]).is_none());
        assert!(covered_warning(&[component("Buy", Some(false)), component("Help", None)]).is_none());
    }

    #[test]
    fn covered_warning_counts_covered_components() {
        let one = covered_warning(&[component("Buy", Some(false)), component("Accept", Some(true))]).unwrap();
        assert_eq!(one.code, "covered_elements");
        assert_eq!(
            one.message,
            "button \"Accept\" is under #cookie-banner, so a click would not reach it."
        );

        let several = covered_warning(&[component("Accept", Some(true)), component("Reject", Some(true))]).unwrap();
        assert!(several
            .message
            .ends_with("and 1 more, so a click would not reach them."));

        let unnamed = covered_warning(&[component("", Some(true))]).unwrap();
        assert!(unnamed.message.contains("\"(no name)\""), "{}", unnamed.message);
    }
}
