//! `traceqa map`: read a page, follow its same-origin links a few levels, and write down which
//! components are on which page.
//!
//! Every page is an ordinary run (`extract::execute_request`); the runs sit in `--out` next to a
//! group folder `g_<id>` (a RunGroup of kind `site`) that holds `group.json` and `site.json`
//! (`spec/site.schema.json`). The crawl rules are in `site.rs`, the robots.txt rules in
//! `robots.rs`. Nothing is clicked or submitted: the only requests are the browser's navigations
//! and one GET of robots.txt.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use clap::Args;
use traceqa_protocol::types::{
    BrowserTarget, ExtractOptions, GroupRun, RunGroup, RunManifest, RunStartParams, RunStatus, SiteComponent, SiteFile,
    SiteLimits, SitePage, SiteRobots, SiteSkip, StorageState, Viewport,
};
use traceqa_recorder::{new_run_id, now_rfc3339, read_components, TargetRef};
use url::Url;

use crate::args::{parse_browser, parse_viewport, DriverArgs};
use crate::extract::execute_request;
use crate::interrupt;
use crate::robots::{self, Robots, Status};
use crate::site::{Config, Frontier, DEFAULT_EXCLUDES, PER_PATTERN};

#[derive(Args)]
pub struct MapArgs {
    /// Page to start from. Only pages with the same scheme, host and port are followed.
    url: String,
    /// How many links deep to follow from the page: 0 reads only the page, 5 is the most.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=5))]
    depth: u32,
    /// Most pages to read, the first one included.
    #[arg(long, value_name = "N", default_value_t = 50, value_parser = clap::value_parser!(u64).range(1..=500))]
    max_pages: u64,
    /// Pause between two pages, in milliseconds, so the site is not hammered.
    #[arg(long, value_name = "MS", default_value_t = 500)]
    delay_ms: u64,
    /// Stop after this many minutes. Pages not yet read are listed as skipped.
    #[arg(long, value_name = "N", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=240))]
    max_minutes: u64,
    /// Follow only paths that match this glob (`*` inside one folder, `**` across folders, `?`
    /// one character). Repeat for several. The first page is always read.
    #[arg(long, value_name = "GLOB")]
    include: Vec<String>,
    /// Never follow paths that match this glob. Repeat for several. Added to the built-in list:
    /// **/logout** **/signout** **/log-out** **/delete** **/unsubscribe**. Following a logout link
    /// with a stored session would sign the session out.
    #[arg(long, value_name = "GLOB")]
    exclude: Vec<String>,
    /// Do not read robots.txt. Say so in site.json. Only for sites you own.
    #[arg(long)]
    no_robots: bool,
    /// Browsers to read each page in, comma separated. Same values as `traceqa extract --browser`.
    #[arg(long, value_delimiter = ',', default_value = "chromium", value_parser = parse_browser)]
    browser: Vec<BrowserTarget>,
    /// Viewport sizes as WIDTHxHEIGHT, comma separated.
    #[arg(long, value_delimiter = ',', default_value = "1280x800", value_parser = parse_viewport)]
    viewport: Vec<Viewport>,
    /// Start every browser with the cookies and stored values in FILE, for pages behind a
    /// sign-in. Make one with `traceqa login`. The file is a secret: keep it out of git.
    #[arg(long, value_name = "FILE")]
    storage_state: Option<PathBuf>,
    /// Where to create the run folders and the group folder.
    #[arg(long, default_value = "runs")]
    out: String,
    /// Show the browser windows.
    #[arg(long)]
    headed: bool,
    /// Print site.json instead of a table. Standard output then holds that one document;
    /// progress and the folders go to standard error.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

/// What is known about one address of the map while the crawl goes on.
struct PageRecord {
    index: usize,
    url: String,
    final_url: Option<String>,
    depth: u32,
    run_id: Option<String>,
    status: String,
    title: Option<String>,
    component_ids: BTreeSet<String>,
    links: BTreeSet<usize>,
    skipped: Option<String>,
}

impl PageRecord {
    /// A page whose components count: it was read and stayed on the origin.
    fn counts(&self) -> bool {
        self.status == "completed" && self.skipped.is_none()
    }
}

/// What the components files of one run showed.
struct PageRead {
    final_url: Option<String>,
    title: Option<String>,
    /// (id, role, name) of every component, once.
    components: BTreeMap<String, (String, String)>,
    /// The href of every link instance, in every frame, hidden ones too.
    hrefs: Vec<String>,
}

pub fn run(args: &MapArgs) -> ExitCode {
    let (seed, template) = match prepare(args) {
        Ok(done) => done,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    let mut exclude: Vec<String> = DEFAULT_EXCLUDES.iter().map(ToString::to_string).collect();
    exclude.extend(args.exclude.iter().cloned());
    let config = Config {
        depth: args.depth,
        max_pages: usize::try_from(args.max_pages).unwrap_or(usize::MAX),
        include: args.include.clone(),
        exclude: exclude.clone(),
    };

    // robots.txt is read once for the origin of the seed (and again if the seed redirects to
    // another origin). The seed itself is subject to it, unless --no-robots.
    let robots = origin_robots(&seed, args.no_robots);
    if !robots.allowed(&path_and_query(&seed)) {
        eprintln!(
            "traceqa: robots.txt of {} does not allow {}. Use --no-robots only on a site you own.",
            seed.origin().ascii_serialization(),
            seed.path()
        );
        return ExitCode::from(2);
    }
    let robots_respected = !args.no_robots;
    let mut robots_status = robots.status;
    let mut frontier = Frontier::new(seed.clone(), config, robots);

    let out = Path::new(&args.out);
    let group_id = format!("g_{}", &new_run_id()[2..]);
    let group_dir = out.join(&group_id);
    let mut group = RunGroup {
        format_version: 1,
        group_id: group_id.clone(),
        kind: "site".to_string(),
        url: String::new(),
        runs: Vec::new(),
        started_at: now_rfc3339(),
        finished_at: None,
    };
    let mut pages: Vec<PageRecord> = Vec::new();
    let mut origin = seed.origin().ascii_serialization();
    let mut seed_shown = String::new();
    let started = Instant::now();
    let limit = Duration::from_secs(args.max_minutes * 60);
    let mut stopped: Option<&'static str> = None;
    let mut refused: Option<ExitCode> = None;
    let mut components: BTreeMap<String, (String, String, BTreeSet<usize>)> = BTreeMap::new();

    while let Some(item) = frontier.pop() {
        if stopped.is_none() {
            stopped = stop_reason(started, limit);
        }
        if stopped.is_none() && item.index > 0 {
            sleep_unless_cancelled(Duration::from_millis(args.delay_ms));
            stopped = stop_reason(started, limit);
        }
        if let Some(reason) = stopped {
            let mut left = vec![item];
            left.extend(frontier.drain());
            for queued in left {
                pages.push(unread(queued.index, &queued.url, queued.depth, reason));
            }
            break;
        }

        interrupt::say(&format!(
            "\nPage {} (depth {}): {}",
            item.index + 1,
            item.depth,
            site_describe(&item.url)
        ));
        let mut request = template.clone();
        request.url = item.url.to_string();
        let (folder, manifest) = match execute_request(request, &args.drivers.drivers, true) {
            Ok(done) => done,
            Err(code) => {
                refused = Some(code);
                break;
            }
        };
        if !group_dir.exists() {
            if let Err(error) = std::fs::create_dir_all(out).and_then(|()| std::fs::create_dir(&group_dir)) {
                eprintln!("traceqa: --out {}: {error}", args.out);
                return ExitCode::from(2);
            }
        }
        if item.index == 0 {
            seed_shown.clone_from(&manifest.url);
            group.url.clone_from(&manifest.url);
        }
        group.runs.push(GroupRun {
            run_id: manifest.run_id.clone(),
            status: manifest.status,
        });

        let mut record = PageRecord {
            index: item.index,
            url: manifest.url.clone(),
            final_url: None,
            depth: item.depth,
            run_id: Some(manifest.run_id.clone()),
            status: manifest.status.to_string(),
            title: None,
            component_ids: BTreeSet::new(),
            links: BTreeSet::new(),
            skipped: None,
        };
        let cancelled = manifest.status == RunStatus::Cancelled;
        if manifest.status == RunStatus::Completed {
            let read = read_page(&folder, &manifest);
            record.final_url.clone_from(&read.final_url);
            record.title.clone_from(&read.title);
            let landed = read.final_url.as_deref().and_then(|text| Url::parse(text).ok());
            if item.index == 0 {
                if let Some(landed) = &landed {
                    if landed.origin() != seed.origin() {
                        let next = origin_robots(landed, args.no_robots);
                        robots_status = next.status;
                        frontier.rebase(landed, next);
                    }
                    origin = landed.origin().ascii_serialization();
                }
            }
            if let Some(landed) = &landed {
                frontier.note_final(item.index, landed);
            }
            if landed
                .as_ref()
                .is_some_and(|landed| landed.origin().ascii_serialization() != origin)
            {
                record.skipped = Some("left-origin".to_string());
            } else {
                // A redirect may have changed the path; the address as asked for is the better
                // base for links when it did not, since the shown one has secrets replaced.
                let base = match &landed {
                    Some(landed) if landed.path() != item.url.path() => landed.clone(),
                    _ => item.url.clone(),
                };
                for (id, (role, name)) in &read.components {
                    record.component_ids.insert(id.clone());
                    components
                        .entry(id.clone())
                        .or_insert_with(|| (role.clone(), name.clone(), BTreeSet::new()))
                        .2
                        .insert(item.index);
                }
                for href in &read.hrefs {
                    if let Some(target) = frontier.offer(&base, href, item.depth) {
                        if target != item.index {
                            record.links.insert(target);
                        }
                    }
                }
            }
        }
        let failed = manifest.status != RunStatus::Completed;
        if item.index == 0 {
            // The first page decides whether there is a map at all.
            if failed {
                refused = Some(if cancelled {
                    ExitCode::from(interrupt::INTERRUPTED)
                } else {
                    ExitCode::FAILURE
                });
            }
        }
        pages.push(record);
        let _ = write_json(&group_dir.join("group.json"), &group);
        let file = build_site(
            &group_id,
            &seed_shown,
            &origin,
            args,
            &exclude,
            robots_respected,
            robots_status,
            &pages,
            &components,
            frontier.skipped(),
        );
        let _ = write_json(&group_dir.join("site.json"), &file);
        if cancelled {
            interrupt::CANCEL.store(true, Ordering::SeqCst);
            let left = frontier.drain();
            for queued in left {
                pages.push(unread(queued.index, &queued.url, queued.depth, "cancelled"));
            }
            break;
        }
        if refused.is_some() {
            break;
        }
    }

    if pages.is_empty() {
        // Nothing was read: there is nothing to leave behind.
        return refused.unwrap_or(ExitCode::from(2));
    }
    group.finished_at = Some(now_rfc3339());
    let file = build_site(
        &group_id,
        &seed_shown,
        &origin,
        args,
        &exclude,
        robots_respected,
        robots_status,
        &pages,
        &components,
        frontier.skipped(),
    );
    let written = write_json(&group_dir.join("site.json"), &file)
        .and_then(|()| write_json(&group_dir.join("group.json"), &group));
    if let Err(error) = written {
        eprintln!("traceqa: cannot write {}: {error}", group_dir.display());
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&file).expect("spec types always serialise")
        );
        eprintln!("Group folder: {}", group_dir.display());
    } else {
        print_summary(&file);
        println!("\nGroup folder: {}", group_dir.display());
    }

    if interrupt::CANCEL.load(Ordering::SeqCst) {
        ExitCode::from(interrupt::INTERRUPTED)
    } else if let Some(code) = refused {
        code
    } else if pages.iter().any(|p| p.status == "failed") {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Checks the arguments and makes the request every page is read with (its url is replaced for
/// each page). The error is the message for a wrong command (exit code 2); nothing exists yet.
fn prepare(args: &MapArgs) -> Result<(Url, RunStartParams), String> {
    traceqa_engine::check_navigable(&args.url)?;
    let seed = Url::parse(&args.url).map_err(|error| format!("not a valid address ({error}): {:?}", args.url))?;
    let mut targets = args.browser.clone();
    for target in &mut targets {
        target.headless = Some(!args.headed);
    }
    let storage_state = args.storage_state.as_deref().map(read_storage_state).transpose()?;
    let custom_drivers = !args.drivers.drivers.is_empty()
        || std::env::var(traceqa_engine::run::DRIVERS_ENV).is_ok_and(|value| !value.trim().is_empty());
    if !custom_drivers {
        if let Some(error) = targets.iter().find_map(|target| target.check_known().err()) {
            return Err(error.to_string());
        }
    }
    for glob in args.include.iter().chain(&args.exclude) {
        if glob.is_empty() {
            return Err("an --include or --exclude glob is empty".to_string());
        }
    }
    let request = RunStartParams {
        feature: "extract".to_string(),
        url: args.url.clone(),
        targets,
        viewports: args.viewport.clone(),
        out_dir: Some(args.out.clone()),
        parallel: None,
        options: Some(ExtractOptions {
            max_scroll_steps: None,
            timeout_ms: None,
            settle_ms: None,
            crops: Some(false),
            include_hidden: Some(true),
            axe: None,
        }),
        storage_state,
    };
    Ok((seed, request))
}

fn origin_robots(url: &Url, disabled: bool) -> Robots {
    if disabled {
        Robots::allow_all(Status::Disabled)
    } else {
        robots::fetch(&url.origin().ascii_serialization())
    }
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

/// An address for progress lines: no query, which can hold a token.
fn site_describe(url: &Url) -> String {
    let mut shown = url.clone();
    shown.set_query(None);
    shown.set_fragment(None);
    shown.to_string()
}

fn stop_reason(started: Instant, limit: Duration) -> Option<&'static str> {
    if interrupt::CANCEL.load(Ordering::SeqCst) {
        Some("cancelled")
    } else if started.elapsed() >= limit {
        Some("max-minutes")
    } else {
        None
    }
}

/// Sleeps in short steps so that Ctrl-C is noticed at once.
fn sleep_unless_cancelled(total: Duration) {
    let end = Instant::now() + total;
    while Instant::now() < end && !interrupt::CANCEL.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20).min(end.saturating_duration_since(Instant::now())));
    }
}

/// A page that was queued and never read.
fn unread(index: usize, url: &Url, depth: u32, reason: &str) -> PageRecord {
    PageRecord {
        index,
        // The query can hold a token and no run has redacted this address.
        url: site_describe(url),
        final_url: None,
        depth,
        run_id: None,
        status: "skipped".to_string(),
        title: None,
        component_ids: BTreeSet::new(),
        links: BTreeSet::new(),
        skipped: Some(reason.to_string()),
    }
}

/// Reads the components files of the completed targets of a run. A target whose file cannot be
/// read is left out, as a target that did not complete is.
fn read_page(folder: &Path, manifest: &RunManifest) -> PageRead {
    let mut read = PageRead {
        final_url: None,
        title: None,
        components: BTreeMap::new(),
        hrefs: Vec::new(),
    };
    for summary in manifest.targets.iter().filter(|t| t.status == RunStatus::Completed) {
        let target = TargetRef {
            dir: summary.dir.clone(),
            path: folder.join(&summary.dir),
            summary: Some(summary.clone()),
        };
        let Ok(file) = read_components(&target) else {
            interrupt::say(&format!(
                "traceqa: could not read {}/{}/components.json",
                folder.display(),
                summary.dir
            ));
            continue;
        };
        if read.final_url.is_none() {
            read.final_url = Some(file.final_url.clone());
            read.title = Some(file.title.clone()).filter(|title| !title.is_empty());
        }
        for component in &file.components {
            read.components
                .entry(component.id.clone())
                .or_insert_with(|| (component.role.clone(), component.name.clone()));
            if component.role == "link" {
                for instance in &component.instances {
                    if let Some(href) = instance.attrs.get("href") {
                        read.hrefs.push(href.clone());
                    }
                }
            }
        }
    }
    read
}

#[allow(clippy::too_many_arguments)]
fn build_site(
    group_id: &str,
    seed: &str,
    origin: &str,
    args: &MapArgs,
    exclude: &[String],
    robots_respected: bool,
    robots_status: Status,
    pages: &[PageRecord],
    components: &BTreeMap<String, (String, String, BTreeSet<usize>)>,
    skipped: &[(String, &'static str)],
) -> SiteFile {
    let counted: Vec<&PageRecord> = pages.iter().filter(|p| p.counts()).collect();
    let mut shared: BTreeSet<String> = BTreeSet::new();
    if counted.len() >= 2 {
        shared = counted[0].component_ids.clone();
        for page in &counted[1..] {
            shared.retain(|id| page.component_ids.contains(id));
        }
    }
    SiteFile {
        format_version: 1,
        group_id: group_id.to_string(),
        seed: seed.to_string(),
        origin: origin.to_string(),
        depth: u64::from(args.depth),
        limits: SiteLimits {
            max_pages: args.max_pages,
            delay_ms: args.delay_ms,
            max_minutes: args.max_minutes,
            per_pattern: PER_PATTERN as u64,
            include: args.include.clone(),
            exclude: exclude.to_vec(),
        },
        robots: SiteRobots {
            respected: robots_respected,
            status: if robots_respected {
                robots_status.as_str().to_string()
            } else {
                Status::Disabled.as_str().to_string()
            },
        },
        pages: pages
            .iter()
            .map(|p| SitePage {
                index: p.index as u64,
                url: p.url.clone(),
                final_url: p.final_url.clone(),
                depth: u64::from(p.depth),
                run_id: p.run_id.clone(),
                status: p.status.clone(),
                title: p.title.clone(),
                component_ids: p.component_ids.iter().cloned().collect(),
                links_out: p.links.iter().map(|&i| i as u64).collect(),
                skipped: p.skipped.clone(),
            })
            .collect(),
        components: components
            .iter()
            .map(|(id, (role, name, on))| {
                (
                    id.clone(),
                    SiteComponent {
                        role: role.clone(),
                        name: name.clone(),
                        pages: on.iter().map(|&i| i as u64).collect(),
                    },
                )
            })
            .collect(),
        shared: shared.into_iter().collect(),
        skipped: skipped
            .iter()
            .map(|(url, reason)| SiteSkip {
                url: url.clone(),
                reason: (*reason).to_string(),
            })
            .collect(),
    }
}

/// The pages, how many components each has, and what was left out.
fn print_summary(file: &SiteFile) {
    println!(
        "\n{:>3} {:>5} {:<10} {:>10}  ADDRESS",
        "#", "DEPTH", "STATUS", "COMPONENTS"
    );
    for page in &file.pages {
        let status = page.skipped.as_deref().map_or(page.status.as_str(), |_| "skipped");
        println!(
            "{:>3} {:>5} {:<10} {:>10}  {}",
            page.index,
            page.depth,
            status,
            page.component_ids.len(),
            page.final_url.as_deref().unwrap_or(&page.url)
        );
    }
    println!(
        "\n{} page{}, {} component{}, {} on every page",
        file.pages.len(),
        if file.pages.len() == 1 { "" } else { "s" },
        file.components.len(),
        if file.components.len() == 1 { "" } else { "s" },
        file.shared.len()
    );
    if !file.skipped.is_empty() {
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for skip in &file.skipped {
            *reasons.entry(skip.reason.as_str()).or_insert(0) += 1;
        }
        let text: Vec<String> = reasons.iter().map(|(reason, n)| format!("{n} {reason}")).collect();
        println!("Links not followed: {} (listed in site.json)", text.join(", "));
    }
    if !file.robots.respected {
        println!("robots.txt was not read (--no-robots).");
    }
}

/// Writes pretty JSON through a temporary file, so a reader never sees half a file.
fn write_json(path: &Path, value: &impl serde::Serialize) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(&serde_json::to_value(value)?)?;
    text.push('\n');
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path))
}

/// Reads a state file. It holds session keys, so an error names the file, never what is in it.
fn read_storage_state(path: &Path) -> Result<StorageState, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|_| {
        format!(
            "{} is not a storage state file: expected cookies and origins, as `traceqa login` saves",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(index: usize, ids: &[&str]) -> PageRecord {
        PageRecord {
            index,
            url: format!("http://s.test/{index}"),
            final_url: None,
            depth: 0,
            run_id: Some(format!("r_{index}")),
            status: "completed".to_string(),
            title: None,
            component_ids: ids.iter().map(ToString::to_string).collect(),
            links: BTreeSet::new(),
            skipped: None,
        }
    }

    #[test]
    fn only_read_pages_that_stayed_on_the_origin_count() {
        let mut off = record(1, &["c_a"]);
        off.skipped = Some("left-origin".to_string());
        let mut failed = record(2, &[]);
        failed.status = "failed".to_string();
        assert!(record(0, &["c_a"]).counts());
        assert!(!off.counts());
        assert!(!failed.counts());
        assert!(!unread(3, &Url::parse("http://s.test/x?token=a").expect("url"), 1, "cancelled").counts());
    }

    #[test]
    fn unread_pages_do_not_show_the_query() {
        let page = unread(
            3,
            &Url::parse("http://s.test/x?token=a#f").expect("url"),
            1,
            "max-minutes",
        );
        assert_eq!(page.url, "http://s.test/x");
        assert_eq!(page.skipped.as_deref(), Some("max-minutes"));
        assert_eq!(page.status, "skipped");
    }
}
