//! The arithmetic behind `traceqa extract --repeat N`: how often each component was found in the
//! runs of a group, and which ones moved. No files, no clock: the caller hands in what each run
//! saw, so the rules can be tested alone.
//!
//! Only targets that completed count. A failed or cancelled target tells nothing about a
//! component, so it neither raises nor lowers a rate: `rate` is `runsPresent / runsCompleted`
//! of that target folder.

use std::collections::{BTreeMap, BTreeSet};

use traceqa_protocol::types::{ComponentStability, InstanceRange, StabilityFile, TargetStability};

/// The version of `stability.json` this code writes.
pub const FORMAT_VERSION: u64 = 1;

pub const FLAG_UNSTABLE: &str = "unstable";
pub const FLAG_COUNT_VARIES: &str = "count_varies";
pub const FLAG_STATE_FLAPS: &str = "state_flaps";
pub const FLAG_NAME_VARIES: &str = "name_varies";

/// One component as one completed target of one run showed it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub id: String,
    pub role: String,
    pub name: String,
    pub instances: u64,
    /// Whether any instance was visible.
    pub visible: bool,
    /// Whether any instance was enabled.
    pub enabled: bool,
}

/// What one run showed: its completed targets by folder name, with their components. A target
/// that did not complete is left out.
pub type RunSeen = BTreeMap<String, Vec<Seen>>;

/// Works out `stability.json` from the runs, oldest first.
pub fn compute(group_id: &str, url: &str, repeat: u64, run_ids: &[String], runs: &[RunSeen]) -> StabilityFile {
    let dirs: BTreeSet<&String> = runs.iter().flat_map(BTreeMap::keys).collect();
    let targets = dirs
        .into_iter()
        .map(|dir| target_stability(dir, &runs.iter().filter_map(|run| run.get(dir)).collect::<Vec<_>>()))
        .collect();
    StabilityFile {
        format_version: FORMAT_VERSION,
        group_id: group_id.to_string(),
        url: url.to_string(),
        repeat,
        runs: run_ids.to_vec(),
        targets,
    }
}

/// What one component looked like in every run that had it, oldest first.
#[derive(Default)]
struct History<'a> {
    seen: Vec<&'a Seen>,
}

fn target_stability(dir: &str, completed: &[&Vec<Seen>]) -> TargetStability {
    let runs_completed = completed.len() as u64;
    let mut by_id: BTreeMap<&str, History> = BTreeMap::new();
    for run in completed {
        for component in run.iter() {
            by_id.entry(component.id.as_str()).or_default().seen.push(component);
        }
    }
    let mut components: Vec<ComponentStability> = by_id
        .into_values()
        .filter_map(|history| component_stability(&history.seen, runs_completed))
        .collect();
    // The steadiest last, so a reader who stops early has seen the movers.
    components.sort_by(|a, b| {
        a.rate
            .total_cmp(&b.rate)
            .then_with(|| a.role.cmp(&b.role))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    let unstable_count = components.iter().filter(|c| !c.flags.is_empty()).count() as u64;
    TargetStability {
        dir: dir.to_string(),
        runs_completed,
        components,
        unstable_count,
    }
}

fn component_stability(seen: &[&Seen], runs_completed: u64) -> Option<ComponentStability> {
    let newest = seen.last()?;
    let runs_present = seen.len() as u64;
    let rate = if runs_completed == 0 {
        0.0
    } else {
        // Four places keep the file readable; the counts above are exact.
        (runs_present as f64 / runs_completed as f64 * 10_000.0).round() / 10_000.0
    };
    let counts: BTreeSet<u64> = seen.iter().map(|s| s.instances).collect();
    let states: BTreeSet<(bool, bool)> = seen.iter().map(|s| (s.visible, s.enabled)).collect();
    let name_variants: Vec<String> = seen
        .iter()
        .map(|s| s.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut flags = Vec::new();
    if runs_present < runs_completed {
        flags.push(FLAG_UNSTABLE.to_string());
    }
    if counts.len() > 1 {
        flags.push(FLAG_COUNT_VARIES.to_string());
    }
    if states.len() > 1 {
        flags.push(FLAG_STATE_FLAPS.to_string());
    }
    if name_variants.len() > 1 {
        flags.push(FLAG_NAME_VARIES.to_string());
    }
    Some(ComponentStability {
        id: newest.id.clone(),
        role: newest.role.clone(),
        name: newest.name.clone(),
        runs_present,
        rate,
        instances: InstanceRange {
            min: counts.iter().next().copied().unwrap_or(0),
            max: counts.iter().next_back().copied().unwrap_or(0),
        },
        name_variants,
        flags,
    })
}

/// Components with at least one flag in any target, least steady first, as (dir, component).
pub fn unstable(file: &StabilityFile) -> Vec<(&str, &ComponentStability)> {
    let mut all: Vec<(&str, &ComponentStability)> = file
        .targets
        .iter()
        .flat_map(|t| {
            t.components
                .iter()
                .filter(|c| !c.flags.is_empty())
                .map(|c| (t.dir.as_str(), c))
        })
        .collect();
    all.sort_by(|a, b| a.1.rate.total_cmp(&b.1.rate));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(id: &str, name: &str, instances: u64, visible: bool, enabled: bool) -> Seen {
        Seen {
            id: id.to_string(),
            role: "button".to_string(),
            name: name.to_string(),
            instances,
            visible,
            enabled,
        }
    }

    fn plain(id: &str) -> Seen {
        seen(id, id, 1, true, true)
    }

    fn run(dir: &str, components: Vec<Seen>) -> RunSeen {
        RunSeen::from([(dir.to_string(), components)])
    }

    fn file(runs: &[RunSeen]) -> StabilityFile {
        let ids: Vec<String> = (0..runs.len()).map(|i| format!("r_{i}")).collect();
        compute("g_x", "https://x.test", runs.len() as u64, &ids, runs)
    }

    fn component<'a>(file: &'a StabilityFile, id: &str) -> &'a ComponentStability {
        file.targets[0]
            .components
            .iter()
            .find(|c| c.id == id)
            .expect("component")
    }

    #[test]
    fn presence_rate_is_runs_present_over_runs_completed() {
        let runs: Vec<RunSeen> = (0..4)
            .map(|i| {
                let mut components = vec![plain("always")];
                if i % 2 == 1 {
                    components.push(plain("flaky"));
                }
                run("chromium-1280x800", components)
            })
            .collect();
        let f = file(&runs);
        assert_eq!(f.targets[0].runs_completed, 4);
        let flaky = component(&f, "flaky");
        assert_eq!((flaky.runs_present, flaky.rate), (2, 0.5));
        assert_eq!(flaky.flags, [FLAG_UNSTABLE]);
        let always = component(&f, "always");
        assert_eq!((always.runs_present, always.rate), (4, 1.0));
        assert!(always.flags.is_empty());
        assert_eq!(f.targets[0].unstable_count, 1);
        // The mover is listed first.
        assert_eq!(f.targets[0].components[0].id, "flaky");
    }

    #[test]
    fn instance_count_variance_is_flagged_with_its_range() {
        let runs = vec![
            run("t", vec![seen("c", "c", 2, true, true)]),
            run("t", vec![seen("c", "c", 5, true, true)]),
            run("t", vec![seen("c", "c", 3, true, true)]),
        ];
        let c = component(&file(&runs), "c").clone();
        assert_eq!((c.instances.min, c.instances.max), (2, 5));
        assert_eq!(c.flags, [FLAG_COUNT_VARIES]);
        assert_eq!(c.rate, 1.0);
    }

    #[test]
    fn a_state_that_differs_between_runs_flaps() {
        let runs = vec![
            run("t", vec![seen("c", "c", 1, true, true)]),
            run("t", vec![seen("c", "c", 1, true, false)]),
        ];
        assert_eq!(component(&file(&runs), "c").flags, [FLAG_STATE_FLAPS]);
        let runs = vec![
            run("t", vec![seen("c", "c", 1, true, true)]),
            run("t", vec![seen("c", "c", 1, false, true)]),
        ];
        assert_eq!(component(&file(&runs), "c").flags, [FLAG_STATE_FLAPS]);
    }

    #[test]
    fn a_name_that_changes_is_listed_and_flagged() {
        let runs = vec![
            run("t", vec![seen("c", "Cart (1)", 1, true, true)]),
            run("t", vec![seen("c", "Cart (2)", 1, true, true)]),
        ];
        let f = file(&runs);
        let c = component(&f, "c");
        assert_eq!(c.name_variants, ["Cart (1)", "Cart (2)"]);
        assert_eq!(c.name, "Cart (2)", "the newest name");
        assert_eq!(c.flags, [FLAG_NAME_VARIES]);
    }

    #[test]
    fn one_run_gives_no_flags() {
        let f = file(&[run("t", vec![plain("a"), seen("b", "b", 3, false, false)])]);
        assert_eq!(f.targets[0].unstable_count, 0);
        assert!(f.targets[0]
            .components
            .iter()
            .all(|c| c.flags.is_empty() && c.rate == 1.0));
    }

    #[test]
    fn unequal_target_sets_use_each_targets_own_denominator() {
        // firefox completed in only two of three runs: a component in both is still 100 %.
        let runs = vec![
            RunSeen::from([
                ("chromium".into(), vec![plain("a")]),
                ("firefox".into(), vec![plain("a")]),
            ]),
            RunSeen::from([("chromium".into(), vec![plain("a")])]),
            RunSeen::from([
                ("chromium".into(), vec![plain("a")]),
                ("firefox".into(), vec![plain("a")]),
            ]),
        ];
        let f = file(&runs);
        let by_dir: BTreeMap<_, _> = f.targets.iter().map(|t| (t.dir.as_str(), t)).collect();
        assert_eq!(by_dir["chromium"].runs_completed, 3);
        assert_eq!(by_dir["firefox"].runs_completed, 2);
        assert_eq!(by_dir["firefox"].components[0].rate, 1.0);
        assert!(by_dir["firefox"].components[0].flags.is_empty());
    }

    #[test]
    fn no_completed_targets_give_no_targets() {
        let f = file(&[RunSeen::new(), RunSeen::new()]);
        assert!(f.targets.is_empty());
        assert_eq!(f.runs.len(), 2);
    }

    #[test]
    fn unstable_lists_the_least_steady_first() {
        let runs = vec![
            run("t", vec![plain("a"), plain("b"), plain("c")]),
            run("t", vec![plain("a"), plain("b")]),
            run("t", vec![plain("a")]),
            run("t", vec![plain("a")]),
        ];
        let f = file(&runs);
        let ids: Vec<&str> = unstable(&f).iter().map(|(_, c)| c.id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
    }
}
