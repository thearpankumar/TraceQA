//! Waiting for a page to finish appearing.
//!
//! A page is not done when it has loaded: a hero that is built by a script, a section that fades in, a list that is
//! filled by a request all show up some time later, and how long it takes changes from one load to the next. Read at
//! once, the same page shows 23 controls one time and 32 the next, and the walk takes them for two screens. So the
//! page is read, left for a moment and read again, until two readings in a row look the same.

use super::model::{StepError, View};

/// The most readings made of one page: with the pause between them, about three seconds.
pub const MAX_READS: usize = 6;

/// The pause between two readings.
pub const PAUSE_MS: u64 = 500;

/// Two readings look the same when at least this share of the ids of the visible components are in both, in
/// thousandths. It is the same share the screens are told apart by, so a change small enough not to make a new screen
/// does not keep the walk waiting, and a ticking name or a rotating banner does not either.
pub const SAME_PER_MILLE: usize = 900;

/// What `settled` reads and waits with.
pub trait Reader {
    fn read(&mut self) -> Result<View, StepError>;
    /// Waits for [`PAUSE_MS`]. An error (a cancel) ends the wait.
    fn pause(&mut self) -> Result<(), StepError>;
}

/// The page as it is when it stops changing, or as it is after `MAX_READS` readings if it never does.
pub fn settled(reader: &mut dyn Reader, max_reads: usize) -> Result<View, StepError> {
    let mut last = reader.read()?;
    for _ in 1..max_reads.max(1) {
        reader.pause()?;
        let now = reader.read()?;
        let same = looks_the_same(&last, &now);
        last = now;
        if same {
            break;
        }
    }
    Ok(last)
}

fn visible_ids(view: &View) -> std::collections::BTreeSet<&str> {
    view.components
        .iter()
        .filter(|c| c.instances.iter().any(|i| i.visible))
        .map(|c| c.id.as_str())
        .collect()
}

/// Whether two readings of a page have at least [`SAME_PER_MILLE`] of their visible ids in common. Two pages with
/// nothing visible on them are the same.
pub fn looks_the_same(a: &View, b: &View) -> bool {
    let (a, b) = (visible_ids(a), visible_ids(b));
    let both = a.intersection(&b).count();
    let either = a.union(&b).count();
    both * 1000 >= either * SAME_PER_MILLE
}

#[cfg(test)]
mod tests {
    use super::super::model::{ViewComponent, ViewInstance};
    use super::*;

    fn view(range: std::ops::Range<u32>) -> View {
        View {
            url: "http://x.test/".into(),
            modal: None,
            frames: vec!["main".into()],
            components: range
                .map(|n| ViewComponent {
                    id: format!("c_{n}"),
                    role: "link".into(),
                    name: format!("n{n}"),
                    path: String::new(),
                    instances: vec![ViewInstance {
                        selector: format!("#n{n}"),
                        frame_id: "main".into(),
                        visible: true,
                        enabled: true,
                        href: None,
                        expanded: None,
                    }],
                })
                .collect(),
        }
    }

    /// Reads what it is given, one view at a time, and counts the pauses.
    struct Script {
        views: Vec<View>,
        reads: usize,
        pauses: usize,
        fail_pause_at: Option<usize>,
    }

    impl Script {
        fn new(views: Vec<View>) -> Self {
            Self {
                views,
                reads: 0,
                pauses: 0,
                fail_pause_at: None,
            }
        }
    }

    impl Reader for Script {
        fn read(&mut self) -> Result<View, StepError> {
            // After the last one the page stays as it is.
            let at = self.reads.min(self.views.len() - 1);
            self.reads += 1;
            Ok(self.views[at].clone())
        }
        fn pause(&mut self) -> Result<(), StepError> {
            self.pauses += 1;
            if self.fail_pause_at == Some(self.pauses) {
                return Err(StepError::Cancelled);
            }
            Ok(())
        }
    }

    #[test]
    fn a_page_that_does_not_change_is_read_twice_and_waited_for_once() {
        let mut page = Script::new(vec![view(0..20)]);
        let got = settled(&mut page, MAX_READS).unwrap();
        assert_eq!(got.components.len(), 20);
        assert_eq!((page.reads, page.pauses), (2, 1));
    }

    #[test]
    fn a_page_that_is_still_appearing_is_read_until_it_stops() {
        // 23 controls, then 26, then 32, then 32 again.
        let mut page = Script::new(vec![view(0..23), view(0..26), view(0..32), view(0..32)]);
        let got = settled(&mut page, MAX_READS).unwrap();
        assert_eq!(got.components.len(), 32, "the page as it ended up");
        assert_eq!((page.reads, page.pauses), (4, 3));
    }

    #[test]
    fn a_page_that_never_stops_is_read_the_most_it_may_be_and_the_last_reading_is_given() {
        let views: Vec<View> = (0..10).map(|n| view(n * 100..n * 100 + 20)).collect();
        let mut page = Script::new(views);
        let got = settled(&mut page, 4).unwrap();
        assert_eq!(page.reads, 4);
        assert_eq!(got.components[0].id, "c_300", "the fourth reading");
    }

    #[test]
    fn a_change_too_small_to_make_a_new_screen_does_not_keep_the_walk_waiting() {
        // One control out of twenty-one differs: 90.5% in common.
        let mut page = Script::new(vec![view(0..20), view(1..21)]);
        settled(&mut page, MAX_READS).unwrap();
        assert_eq!(page.reads, 2);
    }

    #[test]
    fn a_change_a_little_bigger_does_keep_it_waiting() {
        // Two out of twenty-two: 18 of 22 is 81.8%.
        let mut page = Script::new(vec![view(0..20), view(2..22), view(2..22)]);
        settled(&mut page, MAX_READS).unwrap();
        assert_eq!(page.reads, 3);
    }

    #[test]
    fn at_most_one_reading_is_still_one_reading() {
        let mut page = Script::new(vec![view(0..20)]);
        settled(&mut page, 0).unwrap();
        assert_eq!((page.reads, page.pauses), (1, 0));
    }

    #[test]
    fn a_cancel_while_waiting_ends_the_wait() {
        let mut page = Script::new(vec![view(0..20), view(0..30)]);
        page.fail_pause_at = Some(1);
        assert_eq!(settled(&mut page, MAX_READS).unwrap_err(), StepError::Cancelled);
        assert_eq!(page.reads, 1);
    }

    #[test]
    fn two_pages_with_nothing_visible_are_the_same_and_one_with_something_is_not() {
        assert!(looks_the_same(&view(0..0), &view(0..0)));
        assert!(!looks_the_same(&view(0..0), &view(0..5)));
    }

    #[test]
    fn hidden_components_do_not_count() {
        let mut hidden = view(0..20);
        for c in &mut hidden.components[10..] {
            c.instances[0].visible = false;
        }
        assert!(!looks_the_same(&hidden, &view(0..20)));
        assert!(looks_the_same(&hidden, &view(0..10)));
    }
}
