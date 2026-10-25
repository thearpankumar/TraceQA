//! `traceqa runs list|stats|prune`: what is in the runs folder, and clearing out old runs.
//!
//! Pruning deletes people's files, so it is safe by construction. Only a direct child of `--out`
//! is ever considered, and only when its name starts with `r_` or `g_`, it is a plain folder (a
//! symlink or junction is never followed), and it holds a `run.json` (or `group.json`) whose id
//! is the folder's own name. Everything else in `--out` is left alone and never listed. The path
//! is checked again against the canonical `--out` right before it is removed.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{ArgGroup, Args, Subcommand};
use serde_json::json;
use traceqa_protocol::types::{RunGroup, RunManifest, RunStatus};

/// A `running` run older than this is a leftover of a killed process, not a live run.
const STALE_SECS: i64 = 24 * 3600;

/// The largest run.json or group.json read. Real ones are a few kilobytes.
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Args)]
pub struct RunsArgs {
    #[command(subcommand)]
    command: RunsCommand,
}

#[derive(Subcommand)]
enum RunsCommand {
    /// List the runs, newest first.
    List(ListArgs),
    /// Count runs, disk use and dates per page address.
    Stats(StatsArgs),
    /// Delete old runs. Lists what it would delete and does nothing until --yes is given.
    ///
    /// A run is deleted when it is not among the newest N runs of its page (--keep) and/or is
    /// older than the cutoff (--older-than). With both it must satisfy both, so the newest N are
    /// always kept. Only run and group folders directly inside --out are touched.
    Prune(PruneArgs),
}

#[derive(Args)]
struct Scope {
    /// The folder that holds the runs.
    #[arg(long, default_value = "runs")]
    out: String,
    /// Only runs of this page address, exactly as it was asked for.
    #[arg(long, value_name = "URL")]
    url: Option<String>,
}

#[derive(Args)]
struct ListArgs {
    #[command(flatten)]
    scope: Scope,
    /// Print JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct StatsArgs {
    #[command(flatten)]
    scope: Scope,
    /// Print JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
#[command(group(ArgGroup::new("rule").required(true).multiple(true).args(["keep", "older_than"])))]
struct PruneArgs {
    #[command(flatten)]
    scope: Scope,
    /// Keep the newest N runs of each page address.
    #[arg(long, value_name = "N")]
    keep: Option<usize>,
    /// Delete runs started longer ago than this: a number and h, d or w, as 12h, 30d or 2w.
    #[arg(long, value_name = "AGE", value_parser = parse_age)]
    older_than: Option<u64>,
    /// Show what would be deleted and delete nothing.
    #[arg(long)]
    dry_run: bool,
    /// Delete without asking. Without it the plan is printed and the command exits with 2,
    /// because a pipeline has no one to ask.
    #[arg(long)]
    yes: bool,
}

pub fn run(args: &RunsArgs) -> ExitCode {
    match &args.command {
        RunsCommand::List(args) => with_scan(&args.scope, |scan| list(scan, args.json)),
        RunsCommand::Stats(args) => with_scan(&args.scope, |scan| stats(scan, args.json)),
        RunsCommand::Prune(args) => prune(args),
    }
}

// ---------------------------------------------------------------------------------------------
// Reading the folder

struct RunEntry {
    name: String,
    path: PathBuf,
    url: String,
    status: RunStatus,
    started: String,
    epoch: Option<i64>,
    bytes: u64,
    targets: usize,
}

struct GroupEntry {
    name: String,
    path: PathBuf,
    url: String,
    kind: String,
    started: String,
    epoch: Option<i64>,
    bytes: u64,
    members: Vec<String>,
}

#[derive(Default)]
struct Scan {
    runs: Vec<RunEntry>,
    groups: Vec<GroupEntry>,
}

enum Identified {
    Run(Box<RunManifest>),
    Group(Box<RunGroup>),
}

/// True for a symlink, and on Windows for a junction or any other reparse point.
fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Reads a small JSON file that is a plain file (not a link).
fn read_plain_file(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_link(&meta) || !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Decides whether `path` is a run or group folder named `name`. This is the only gate to
/// deletion: anything it does not recognise is not ours.
fn identify(path: &Path, name: &str) -> Option<Identified> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_link(&meta) || !meta.is_dir() {
        return None;
    }
    if name.starts_with("r_") {
        let manifest: RunManifest = serde_json::from_str(&read_plain_file(&path.join("run.json"))?).ok()?;
        (manifest.run_id == name).then(|| Identified::Run(Box::new(manifest)))
    } else if name.starts_with("g_") {
        let group: RunGroup = serde_json::from_str(&read_plain_file(&path.join("group.json"))?).ok()?;
        (group.group_id == name).then(|| Identified::Group(Box::new(group)))
    } else {
        None
    }
}

/// Bytes under a folder, not following links.
fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                return 0;
            };
            if is_link(&meta) {
                0
            } else if meta.is_dir() {
                dir_bytes(&entry.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

fn scan(out: &Path) -> Result<Scan, String> {
    let mut found = Scan::default();
    let entries = match fs::read_dir(out) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(e) => return Err(format!("--out {}: {e}", out.display())),
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let path = entry.path();
        match identify(&path, &name) {
            Some(Identified::Run(manifest)) => found.runs.push(RunEntry {
                bytes: dir_bytes(&path),
                epoch: parse_rfc3339(&manifest.started_at),
                targets: manifest.targets.len(),
                name,
                path,
                url: manifest.url,
                status: manifest.status,
                started: manifest.started_at,
            }),
            Some(Identified::Group(group)) => found.groups.push(GroupEntry {
                bytes: dir_bytes(&path),
                epoch: parse_rfc3339(&group.started_at),
                members: group.runs.into_iter().map(|r| r.run_id).collect(),
                name,
                path,
                url: group.url,
                kind: group.kind,
                started: group.started_at,
            }),
            None => {}
        }
    }
    // Newest first; the name breaks ties, so the order never depends on the file system.
    found
        .runs
        .sort_by(|a, b| (b.epoch, &b.started, &b.name).cmp(&(a.epoch, &a.started, &a.name)));
    found
        .groups
        .sort_by(|a, b| (b.epoch, &b.started, &b.name).cmp(&(a.epoch, &a.started, &a.name)));
    Ok(found)
}

fn with_scan(scope: &Scope, show: impl FnOnce(&Scan) -> ExitCode) -> ExitCode {
    match scan(Path::new(&scope.out)) {
        Ok(mut scan) => {
            if let Some(url) = &scope.url {
                scan.runs.retain(|r| &r.url == url);
                scan.groups.retain(|g| &g.url == url);
            }
            show(&scan)
        }
        Err(why) => {
            eprintln!("traceqa: {why}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// list and stats

fn list(scan: &Scan, as_json: bool) -> ExitCode {
    if as_json {
        let runs: Vec<_> = scan
            .runs
            .iter()
            .map(|r| {
                json!({"runId": r.name, "url": r.url, "status": r.status.to_string(),
                       "startedAt": r.started, "targets": r.targets, "bytes": r.bytes})
            })
            .collect();
        let groups: Vec<_> = scan
            .groups
            .iter()
            .map(|g| {
                json!({"groupId": g.name, "kind": g.kind, "url": g.url, "runs": g.members.len(),
                       "startedAt": g.started, "bytes": g.bytes})
            })
            .collect();
        print_json(&json!({"runs": runs, "groups": groups}));
        return ExitCode::SUCCESS;
    }
    if scan.runs.is_empty() && scan.groups.is_empty() {
        println!("No runs.");
        return ExitCode::SUCCESS;
    }
    if !scan.runs.is_empty() {
        println!(
            "{:<24} {:<10} {:<20} {:>7} {:>9}  URL",
            "RUN", "STATUS", "STARTED", "TARGETS", "SIZE"
        );
        for r in &scan.runs {
            println!(
                "{:<24} {:<10} {:<20} {:>7} {:>9}  {}",
                r.name,
                r.status.to_string(),
                r.started,
                r.targets,
                format_bytes(r.bytes),
                r.url
            );
        }
    }
    if !scan.groups.is_empty() {
        println!(
            "\n{:<24} {:<10} {:<20} {:>7} {:>9}  URL",
            "GROUP", "KIND", "STARTED", "RUNS", "SIZE"
        );
        for g in &scan.groups {
            println!(
                "{:<24} {:<10} {:<20} {:>7} {:>9}  {}",
                g.name,
                g.kind,
                g.started,
                g.members.len(),
                format_bytes(g.bytes),
                g.url
            );
        }
    }
    ExitCode::SUCCESS
}

#[derive(Default)]
struct UrlStats {
    runs: u64,
    by_status: BTreeMap<String, u64>,
    groups: u64,
    bytes: u64,
    /// (epoch, text) of the oldest and newest start.
    oldest: Option<(Option<i64>, String)>,
    newest: Option<(Option<i64>, String)>,
}

impl UrlStats {
    fn note_start(&mut self, epoch: Option<i64>, text: &str) {
        let key = (epoch, text.to_string());
        if self.oldest.as_ref().is_none_or(|old| key < *old) {
            self.oldest = Some(key.clone());
        }
        if self.newest.as_ref().is_none_or(|new| key > *new) {
            self.newest = Some(key);
        }
    }
}

fn stats(scan: &Scan, as_json: bool) -> ExitCode {
    let mut per_url: BTreeMap<&str, UrlStats> = BTreeMap::new();
    for r in &scan.runs {
        let s = per_url.entry(&r.url).or_default();
        s.runs += 1;
        *s.by_status.entry(r.status.to_string()).or_default() += 1;
        s.bytes += r.bytes;
        s.note_start(r.epoch, &r.started);
    }
    for g in &scan.groups {
        let s = per_url.entry(&g.url).or_default();
        s.groups += 1;
        s.bytes += g.bytes;
    }
    let count = |s: &UrlStats, status: &str| s.by_status.get(status).copied().unwrap_or(0);
    let text = |t: &Option<(Option<i64>, String)>| t.as_ref().map(|t| t.1.clone());
    if as_json {
        let urls: Vec<_> = per_url
            .iter()
            .map(|(url, s)| {
                json!({"url": url, "runs": s.runs, "status": s.by_status, "groups": s.groups,
                       "bytes": s.bytes, "oldest": text(&s.oldest), "newest": text(&s.newest)})
            })
            .collect();
        print_json(&json!({
            "urls": urls,
            "totalRuns": per_url.values().map(|s| s.runs).sum::<u64>(),
            "totalBytes": per_url.values().map(|s| s.bytes).sum::<u64>(),
        }));
        return ExitCode::SUCCESS;
    }
    if per_url.is_empty() {
        println!("No runs.");
        return ExitCode::SUCCESS;
    }
    println!(
        "{:>5} {:>9} {:>6} {:>9} {:>7} {:>6} {:>9}  {:<20} {:<20}  URL",
        "RUNS", "COMPLETED", "FAILED", "CANCELLED", "RUNNING", "GROUPS", "SIZE", "OLDEST", "NEWEST"
    );
    for (url, s) in &per_url {
        println!(
            "{:>5} {:>9} {:>6} {:>9} {:>7} {:>6} {:>9}  {:<20} {:<20}  {url}",
            s.runs,
            count(s, "completed"),
            count(s, "failed"),
            count(s, "cancelled"),
            count(s, "running"),
            s.groups,
            format_bytes(s.bytes),
            text(&s.oldest).unwrap_or_default(),
            text(&s.newest).unwrap_or_default(),
        );
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------
// prune

/// One folder the plan removes.
#[derive(Debug, PartialEq, Eq)]
struct Doomed {
    name: String,
    path: PathBuf,
    bytes: u64,
    why: String,
}

#[derive(Default, Debug)]
struct Plan {
    runs: Vec<Doomed>,
    groups: Vec<Doomed>,
    /// Runs left alone although a rule matched, and why. Also stale `running` notes.
    notes: Vec<String>,
}

/// Decides what to delete. `cutoff` is an age in seconds; `now` is seconds since 1970.
fn plan(scan: &Scan, keep: Option<usize>, cutoff: Option<u64>, url: Option<&str>, now: i64) -> Plan {
    let mut plan = Plan::default();
    let in_scope = |u: &str| url.is_none_or(|wanted| wanted == u);
    let older = |epoch: i64| cutoff.is_none_or(|c| now.saturating_sub(epoch) > i64::try_from(c).unwrap_or(i64::MAX));

    let mut per_url: BTreeMap<&str, Vec<&RunEntry>> = BTreeMap::new();
    for run in scan.runs.iter().filter(|r| in_scope(&r.url)) {
        if run.epoch.is_none() {
            plan.notes.push(format!(
                "{}: kept, startedAt {:?} is not a timestamp",
                run.name, run.started
            ));
            continue;
        }
        per_url.entry(&run.url).or_default().push(run);
    }

    let mut doomed_names: HashSet<&str> = HashSet::new();
    for runs in per_url.values_mut() {
        runs.sort_by(|a, b| (b.epoch, &b.name).cmp(&(a.epoch, &a.name)));
        for (rank, run) in runs.iter().enumerate() {
            let Some(started) = run.epoch else { continue };
            let age = now.saturating_sub(started);
            let mut stale = false;
            if run.status == RunStatus::Running {
                if age <= STALE_SECS {
                    plan.notes.push(format!("{}: kept, it is still running", run.name));
                    continue;
                }
                stale = true;
            }
            let outside_keep = keep.is_none_or(|n| rank >= n);
            if outside_keep && older(started) {
                if stale {
                    plan.notes.push(format!(
                        "{}: marked running but started over 24 hours ago; treated as stale",
                        run.name
                    ));
                }
                let mut why = Vec::new();
                if let Some(n) = keep {
                    why.push(format!("not among the newest {n}"));
                }
                if cutoff.is_some() {
                    why.push(format!("started {}", age_text(age)));
                }
                doomed_names.insert(&run.name);
                plan.runs.push(Doomed {
                    name: run.name.clone(),
                    path: run.path.clone(),
                    bytes: run.bytes,
                    why: why.join(", "),
                });
            }
        }
    }

    // A group goes when none of its member runs is left on disk.
    let on_disk: HashSet<&str> = scan.runs.iter().map(|r| r.name.as_str()).collect();
    for group in scan.groups.iter().filter(|g| in_scope(&g.url)) {
        let gone = |m: &String| !on_disk.contains(m.as_str()) || doomed_names.contains(m.as_str());
        let removable = if group.members.is_empty() {
            // Being written right now, or abandoned: only an old one goes.
            group
                .epoch
                .is_some_and(|started| now.saturating_sub(started) > STALE_SECS && older(started))
        } else {
            group.members.iter().all(gone)
        };
        if removable {
            plan.groups.push(Doomed {
                name: group.name.clone(),
                path: group.path.clone(),
                bytes: group.bytes,
                why: "all its runs are gone".to_string(),
            });
        }
    }
    plan
}

fn prune(args: &PruneArgs) -> ExitCode {
    let out = Path::new(&args.scope.out);
    let found = match scan(out) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    let now = now_epoch();
    let plan = plan(&found, args.keep, args.older_than, args.scope.url.as_deref(), now);
    for note in &plan.notes {
        eprintln!("traceqa: {note}");
    }
    if plan.runs.is_empty() && plan.groups.is_empty() {
        println!("Nothing to prune.");
        return ExitCode::SUCCESS;
    }
    let total: u64 = plan.runs.iter().chain(&plan.groups).map(|d| d.bytes).sum();
    let verb = if args.yes && !args.dry_run {
        "Deleting"
    } else {
        "Would delete"
    };
    println!(
        "{verb} {} run(s) and {} group(s), {}:",
        plan.runs.len(),
        plan.groups.len(),
        format_bytes(total)
    );
    for d in plan.runs.iter().chain(&plan.groups) {
        println!("  {:<24} {:>9}  {}", d.name, format_bytes(d.bytes), d.why);
    }
    if args.dry_run {
        return ExitCode::SUCCESS;
    }
    if !args.yes {
        eprintln!("traceqa: nothing deleted. Run again with --yes to delete these, or --dry-run to only look.");
        return ExitCode::from(2);
    }
    let Ok(out_canonical) = fs::canonicalize(out) else {
        eprintln!("traceqa: --out {}: cannot be resolved", out.display());
        return ExitCode::from(2);
    };
    let mut failed = 0;
    let mut freed = 0;
    for d in plan.runs.iter().chain(&plan.groups) {
        match delete_folder(&out_canonical, &d.path, &d.name) {
            Ok(()) => freed += d.bytes,
            Err(why) => {
                failed += 1;
                eprintln!("traceqa: could not delete {}: {why}", d.name);
            }
        }
    }
    println!("Freed {}.", format_bytes(freed));
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Removes one run or group folder after proving again that it is one and that it sits directly
/// inside `out_canonical`.
fn delete_folder(out_canonical: &Path, path: &Path, name: &str) -> Result<(), String> {
    if identify(path, name).is_none() {
        return Err("it is no longer a run folder".to_string());
    }
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if canonical.parent() != Some(out_canonical) || canonical.file_name().and_then(|n| n.to_str()) != Some(name) {
        return Err("it is not directly inside --out".to_string());
    }
    if fs::remove_dir_all(&canonical).is_ok() {
        return Ok(());
    }
    // Windows refuses to delete read-only files; clear the attribute and try once more.
    clear_read_only(&canonical);
    fs::remove_dir_all(&canonical).map_err(|e| e.to_string())
}

/// Clears the read-only attribute below `path`, without following links.
#[allow(clippy::permissions_set_readonly_false)]
fn clear_read_only(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else { return };
    if is_link(&meta) {
        return;
    }
    if meta.permissions().readonly() {
        let mut permissions = meta.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
    if meta.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                clear_read_only(&entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers

fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// `12h`, `30d` or `2w` as seconds.
fn parse_age(text: &str) -> Result<u64, String> {
    let wrong = || format!("{text:?} is not an age: use a number and h, d or w, as 12h, 30d or 2w");
    let split = text
        .len()
        .checked_sub(1)
        .filter(|&i| text.is_char_boundary(i))
        .ok_or_else(wrong)?;
    let (number, unit) = text.split_at(split);
    let seconds = match unit {
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(wrong()),
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(wrong());
    }
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(seconds))
        .ok_or_else(wrong)
}

/// Seconds since 1970 of `YYYY-MM-DDTHH:MM:SS` followed by optional fractional seconds and `Z`.
/// Run files only ever hold this UTC form; anything else is `None`.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.splitn(3, '-');
    let (year, month, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut t = time.splitn(3, ':');
    let (hour, minute, second): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn age_text(seconds: i64) -> String {
    match seconds {
        s if s < 3600 => format!("{} min ago", s.max(0) / 60),
        s if s < 2 * 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn run(name: &str, url: &str, age_secs: i64, status: RunStatus) -> RunEntry {
        RunEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            url: url.to_string(),
            status,
            started: "x".to_string(),
            epoch: Some(NOW - age_secs),
            bytes: 10,
            targets: 1,
        }
    }

    fn scan_of(runs: Vec<RunEntry>) -> Scan {
        Scan { runs, groups: vec![] }
    }

    fn names(doomed: &[Doomed]) -> Vec<&str> {
        let mut names: Vec<&str> = doomed.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        names
    }

    const DAY: i64 = 86_400;

    #[test]
    fn ages_parse_and_bad_ones_are_refused() {
        assert_eq!(parse_age("12h"), Ok(12 * 3600));
        assert_eq!(parse_age("30d"), Ok(30 * 86_400));
        assert_eq!(parse_age("2w"), Ok(14 * 86_400));
        for bad in ["5x", "d", "", "-3d", "1.5d", "30", "3 d", "99999999999999999999d", "ä"] {
            assert!(parse_age(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn timestamps_parse() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2023-11-14T22:13:20Z"), Some(1_700_000_000));
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00.500Z"), Some(951_782_400));
        assert_eq!(parse_rfc3339("yesterday"), None);
        assert_eq!(parse_rfc3339("2023-13-14T22:13:20Z"), None);
        assert_eq!(parse_rfc3339("2023-11-14T22:13:20+02:00"), None);
    }

    #[test]
    fn keep_protects_the_newest_n_of_each_url() {
        let scan = scan_of(vec![
            run("r_a1", "A", DAY, RunStatus::Completed),
            run("r_a2", "A", 2 * DAY, RunStatus::Completed),
            run("r_a3", "A", 3 * DAY, RunStatus::Completed),
            run("r_b1", "B", 9 * DAY, RunStatus::Completed),
        ]);
        let p = plan(&scan, Some(2), None, None, NOW);
        assert_eq!(names(&p.runs), ["r_a3"]);
    }

    #[test]
    fn older_than_alone_uses_the_cutoff() {
        let scan = scan_of(vec![
            run("r_new", "A", DAY, RunStatus::Completed),
            run("r_old", "A", 40 * DAY, RunStatus::Completed),
        ]);
        let p = plan(&scan, None, Some(30 * 86_400), None, NOW);
        assert_eq!(names(&p.runs), ["r_old"]);
    }

    #[test]
    fn both_rules_must_hold_so_the_newest_n_are_protected() {
        let scan = scan_of(vec![
            run("r_1", "A", 40 * DAY, RunStatus::Completed),
            run("r_2", "A", 50 * DAY, RunStatus::Completed),
            run("r_3", "A", 60 * DAY, RunStatus::Completed),
            run("r_4", "A", DAY, RunStatus::Completed),
        ]);
        // r_4 is newest and young; r_1 is old but within the newest 2.
        let p = plan(&scan, Some(2), Some(30 * 86_400), None, NOW);
        assert_eq!(names(&p.runs), ["r_2", "r_3"]);
    }

    #[test]
    fn a_running_run_is_kept_and_a_stale_one_is_reported() {
        let scan = scan_of(vec![
            run("r_live", "A", 3600, RunStatus::Running),
            run("r_stale", "A", 3 * DAY, RunStatus::Running),
        ]);
        let p = plan(&scan, Some(0), None, None, NOW);
        assert_eq!(names(&p.runs), ["r_stale"]);
        assert!(p.notes.iter().any(|n| n.contains("r_live") && n.contains("running")));
        assert!(p.notes.iter().any(|n| n.contains("r_stale") && n.contains("stale")));
    }

    #[test]
    fn the_url_filter_limits_what_is_deleted() {
        let scan = scan_of(vec![
            run("r_a", "A", 5 * DAY, RunStatus::Completed),
            run("r_b", "B", 5 * DAY, RunStatus::Completed),
        ]);
        let p = plan(&scan, Some(0), None, Some("A"), NOW);
        assert_eq!(names(&p.runs), ["r_a"]);
    }

    #[test]
    fn a_group_goes_with_its_last_run_and_not_before() {
        let mut scan = scan_of(vec![
            run("r_1", "A", 5 * DAY, RunStatus::Completed),
            run("r_2", "A", DAY, RunStatus::Completed),
        ]);
        let group = |name: &str, members: &[&str]| GroupEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            url: "A".to_string(),
            kind: "repeat".to_string(),
            started: "x".to_string(),
            epoch: Some(NOW - 5 * DAY),
            bytes: 1,
            members: members.iter().map(ToString::to_string).collect(),
        };
        scan.groups = vec![
            group("g_both", &["r_1", "r_2"]),
            group("g_old", &["r_1", "r_missing"]),
            group("g_empty_old", &[]),
        ];
        let p = plan(&scan, Some(1), None, None, NOW);
        assert_eq!(names(&p.runs), ["r_1"]);
        assert_eq!(names(&p.groups), ["g_empty_old", "g_old"]);
    }

    #[test]
    fn a_run_with_an_unreadable_date_is_never_deleted() {
        let mut odd = run("r_odd", "A", 99 * DAY, RunStatus::Completed);
        odd.epoch = None;
        let p = plan(&scan_of(vec![odd]), Some(0), Some(1), None, NOW);
        assert!(p.runs.is_empty());
        assert_eq!(p.notes.len(), 1);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
