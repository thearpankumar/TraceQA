//! The crawl logic of `traceqa map`: which links are followed and in which order. No network, no
//! browser and no files here, so every rule can be tested alone. `map.rs` drives it.
//!
//! The rules, in the order a link meets them:
//!
//! 1. It is resolved against its page, the fragment is dropped, and tracking parameters
//!    (`utm_*`, `fbclid`, ...) are removed from the query. Other parameters stay: `?page=2` is
//!    another page.
//! 2. Non-web schemes (`mailto:`, `tel:`, `javascript:`, `data:`), and links to files that are
//!    not pages (documents, archives, images, media), are skipped.
//! 3. Only the origin of the seed (scheme, host and port) is followed.
//! 4. An address that is already known is not read again.
//! 5. `--exclude` globs, `--include` globs, then robots.txt.
//! 6. The depth limit, then at most [`PER_PATTERN`] addresses per path pattern (so that
//!    `/item/1`, `/item/2`, ... and calendars cannot go on for ever), then `--max-pages`.

use std::collections::{HashMap, HashSet, VecDeque};

use url::Url;

use crate::robots::Robots;

/// Most addresses read for one path pattern.
pub const PER_PATTERN: usize = 20;

/// Globs excluded unless `--include` names them: links that change state on a GET. Following
/// `/logout` with a stored session would sign the session out; `/delete` can delete.
pub const DEFAULT_EXCLUDES: [&str; 5] = [
    "**/logout**",
    "**/signout**",
    "**/log-out**",
    "**/delete**",
    "**/unsubscribe**",
];

/// Query parameters that only track where a visitor came from.
const TRACKERS: [&str; 8] = [
    "fbclid", "gclid", "dclid", "msclkid", "mc_cid", "mc_eid", "igshid", "_ga",
];

/// Path extensions of things that are not pages.
const FILE_EXTENSIONS: &[&str] = &[
    "pdf", "zip", "gz", "tgz", "tar", "rar", "7z", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "exe", "dmg",
    "msi", "apk", "iso", "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif", "mp3", "mp4", "webm", "mov",
    "avi", "wav", "ogg", "woff", "woff2", "ttf", "epub", "xml", "json",
];

/// How the crawl is bounded.
#[derive(Debug, Clone)]
pub struct Config {
    pub depth: u32,
    pub max_pages: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// A page waiting to be read.
#[derive(Debug, Clone)]
pub struct Queued {
    pub index: usize,
    pub url: Url,
    pub depth: u32,
}

/// The pages found so far and the ones still to read.
pub struct Frontier {
    config: Config,
    origin: url::Origin,
    robots: Robots,
    known: HashMap<String, usize>,
    patterns: HashMap<String, usize>,
    queue: VecDeque<Queued>,
    next_index: usize,
    skipped: Vec<(String, &'static str)>,
    skipped_seen: HashSet<(String, &'static str)>,
}

/// Most skipped links listed in site.json.
const MAX_SKIPPED: usize = 2000;

impl Frontier {
    /// A frontier holding only the seed, page 0.
    pub fn new(seed: Url, config: Config, robots: Robots) -> Self {
        let mut frontier = Frontier {
            origin: seed.origin(),
            config,
            robots,
            known: HashMap::new(),
            patterns: HashMap::new(),
            queue: VecDeque::new(),
            next_index: 0,
            skipped: Vec::new(),
            skipped_seen: HashSet::new(),
        };
        frontier.known.insert(key(&seed), 0);
        *frontier.patterns.entry(pattern_of(&seed)).or_insert(0) += 1;
        frontier.queue.push_back(Queued {
            index: 0,
            url: seed,
            depth: 0,
        });
        frontier.next_index = 1;
        frontier
    }

    /// The seed redirected: the origin it ended on is the one to follow, with its own robots.txt.
    pub fn rebase(&mut self, final_url: &Url, robots: Robots) {
        self.origin = final_url.origin();
        self.robots = robots;
    }

    /// The next page to read, in the order the pages were found.
    pub fn pop(&mut self) -> Option<Queued> {
        self.queue.pop_front()
    }

    /// The pages still waiting, emptying the queue.
    pub fn drain(&mut self) -> Vec<Queued> {
        self.queue.drain(..).collect()
    }

    /// Where a page ended up after redirects: a later link to that address is the same page.
    pub fn note_final(&mut self, index: usize, final_url: &Url) {
        self.known.entry(key(final_url)).or_insert(index);
    }

    pub fn skipped(&self) -> &[(String, &'static str)] {
        &self.skipped
    }

    fn skip(&mut self, url: String, reason: &'static str) -> Option<usize> {
        if self.skipped.len() < MAX_SKIPPED && self.skipped_seen.insert((url.clone(), reason)) {
            self.skipped.push((url, reason));
        }
        None
    }

    /// Offers one link found on a page `base` that is `parent_depth` links from the seed.
    /// Returns the index of the page it leads to when it is (or now is) a page of the map.
    pub fn offer(&mut self, base: &Url, href: &str, parent_depth: u32) -> Option<usize> {
        let target = match resolve(base, href) {
            Ok(target) => target,
            Err(reason) => return self.skip(describe_raw(href), reason),
        };
        let shown = describe(&target);
        if is_file(&target) {
            return self.skip(shown, "file");
        }
        if target.origin() != self.origin {
            return self.skip(shown, "other-origin");
        }
        if let Some(&index) = self.known.get(&key(&target)) {
            return Some(index);
        }
        let path = target.path().to_string();
        if self.config.exclude.iter().any(|glob| glob_matches(glob, &path)) {
            return self.skip(shown, "excluded");
        }
        if !self.config.include.is_empty() && !self.config.include.iter().any(|glob| glob_matches(glob, &path)) {
            return self.skip(shown, "not-included");
        }
        let with_query = match target.query() {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        if !self.robots.allowed(&with_query) {
            return self.skip(shown, "robots");
        }
        if parent_depth + 1 > self.config.depth {
            // Too deep: not listed, as every page at the last level would list its whole menu.
            return None;
        }
        let pattern = pattern_of(&target);
        if self.patterns.get(&pattern).copied().unwrap_or(0) >= PER_PATTERN {
            return self.skip(shown, "pattern-limit");
        }
        if self.next_index >= self.config.max_pages {
            return self.skip(shown, "max-pages");
        }
        *self.patterns.entry(pattern).or_insert(0) += 1;
        let index = self.next_index;
        self.next_index += 1;
        self.known.insert(key(&target), index);
        self.queue.push_back(Queued {
            index,
            url: target,
            depth: parent_depth + 1,
        });
        Some(index)
    }
}

/// The identity of an address: the address without its fragment.
fn key(url: &Url) -> String {
    url.as_str().to_string()
}

/// Resolves `href` against `base` into an address to consider, or says why it cannot be one.
pub fn resolve(base: &Url, href: &str) -> Result<Url, &'static str> {
    let href = href.trim();
    if href.is_empty() {
        return Ok(strip(base.clone()));
    }
    // A scheme-like start that is not http(s) is judged by name, whatever the base is.
    if let Some((scheme, _)) = href.split_once(':') {
        let scheme = scheme.to_ascii_lowercase();
        let looks_like_scheme = !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic());
        if looks_like_scheme && scheme != "http" && scheme != "https" {
            return Err("scheme");
        }
    }
    let joined = base.join(href).map_err(|_| "invalid")?;
    if !matches!(joined.scheme(), "http" | "https") {
        return Err("scheme");
    }
    if !joined.username().is_empty() || joined.password().is_some() {
        return Err("invalid");
    }
    Ok(strip(joined))
}

/// Drops the fragment and the tracking parameters.
fn strip(mut url: Url) -> Url {
    url.set_fragment(None);
    if let Some(query) = url.query().map(str::to_string) {
        let kept: Vec<&str> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter(|pair| !is_tracker(pair.split('=').next().unwrap_or("")))
            .collect();
        if kept.is_empty() {
            url.set_query(None);
        } else {
            url.set_query(Some(&kept.join("&")));
        }
    }
    url
}

fn is_tracker(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || TRACKERS.contains(&name.as_str())
}

fn is_file(url: &Url) -> bool {
    let last = url.path().rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, extension)) => FILE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// How a skipped web address is written down: no query and no fragment, which can hold tokens.
fn describe(url: &Url) -> String {
    let mut shown = url.clone();
    shown.set_query(None);
    shown.set_fragment(None);
    shown.to_string()
}

/// A link that is not a web address, or could not be read, shortened.
fn describe_raw(href: &str) -> String {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return href.split(['?', '#']).next().unwrap_or("").chars().take(200).collect();
    }
    // mailto:a@b.c?subject=... and javascript:... could say anything: keep the scheme and the start.
    href.split(['?', '#']).next().unwrap_or("").chars().take(60).collect()
}

/// A path pattern for the per-pattern cap: numbers, long hex strings and UUIDs in the path
/// become placeholders, and only the names of query parameters count, not their values.
pub fn pattern_of(url: &Url) -> String {
    let segments: Vec<String> = url
        .path()
        .split('/')
        .map(|segment| {
            if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
                "{n}".to_string()
            } else if segment.len() >= 8
                && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                && segment.chars().any(|c| c.is_ascii_digit())
            {
                "{h}".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect();
    let mut names: Vec<String> = url
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split('=').next().unwrap_or("").to_string())
        .collect();
    names.sort();
    names.dedup();
    let mut pattern = segments.join("/");
    if !names.is_empty() {
        pattern.push('?');
        pattern.push_str(&names.join("&"));
    }
    pattern
}

/// Shell-style match on a path: `**` any run of characters, `*` any run inside one segment,
/// `?` one character inside a segment.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    fn go(glob: &[char], path: &[char]) -> bool {
        match glob.first() {
            None => path.is_empty(),
            Some('*') if glob.get(1) == Some(&'*') => {
                let rest = &glob[2..];
                (0..=path.len()).any(|skip| go(rest, &path[skip..]))
            }
            Some('*') => {
                let rest = &glob[1..];
                let mut skip = 0;
                loop {
                    if go(rest, &path[skip..]) {
                        return true;
                    }
                    if skip < path.len() && path[skip] != '/' {
                        skip += 1;
                    } else {
                        return false;
                    }
                }
            }
            Some('?') => matches!(path.first(), Some(c) if *c != '/') && go(&glob[1..], &path[1..]),
            Some(c) => path.first() == Some(c) && go(&glob[1..], &path[1..]),
        }
    }
    let glob: Vec<char> = glob.chars().collect();
    let path: Vec<char> = path.chars().collect();
    go(&glob, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("test address")
    }

    fn config() -> Config {
        Config {
            depth: 2,
            max_pages: 50,
            include: Vec::new(),
            exclude: DEFAULT_EXCLUDES.iter().map(ToString::to_string).collect(),
        }
    }

    fn frontier() -> Frontier {
        Frontier::new(
            url("http://site.test/"),
            config(),
            Robots::allow_all(crate::robots::Status::NotFound),
        )
    }

    #[test]
    fn links_resolve_against_their_page_and_lose_fragments_and_trackers() {
        let base = url("http://site.test/a/b");
        assert_eq!(resolve(&base, "c#top").unwrap().as_str(), "http://site.test/a/c");
        assert_eq!(
            resolve(&base, "/x?utm_source=n&id=3&fbclid=z").unwrap().as_str(),
            "http://site.test/x?id=3"
        );
        assert_eq!(
            resolve(&base, "/x?utm_source=n").unwrap().as_str(),
            "http://site.test/x"
        );
        assert_eq!(
            resolve(&base, "//other.test/p").unwrap().as_str(),
            "http://other.test/p"
        );
        assert_eq!(resolve(&base, "").unwrap().as_str(), "http://site.test/a/b");
        assert_eq!(resolve(&base, "#section").unwrap().as_str(), "http://site.test/a/b");
    }

    #[test]
    fn non_web_schemes_are_refused_by_name() {
        let base = url("http://site.test/");
        for href in [
            "mailto:a@b.c",
            "tel:+123",
            "javascript:void(0)",
            "data:text/html,x",
            "JavaScript:alert(1)",
            "ftp://x/y",
        ] {
            assert_eq!(resolve(&base, href), Err("scheme"), "{href}");
        }
        assert_eq!(resolve(&base, "https://u:p@site.test/"), Err("invalid"));
    }

    #[test]
    fn same_origin_means_scheme_host_and_port() {
        let mut f = frontier();
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "https://site.test/", 0), None, "scheme differs");
        assert_eq!(f.offer(&base, "http://site.test:8080/", 0), None, "port differs");
        assert_eq!(f.offer(&base, "http://www.site.test/", 0), None, "host differs");
        assert_eq!(
            f.offer(&base, "http://site.test:80/about", 0),
            Some(1),
            "the default port is the same origin"
        );
        assert!(f.skipped().iter().all(|(_, why)| *why == "other-origin"));
        assert_eq!(f.skipped().len(), 3);
    }

    #[test]
    fn files_are_skipped_whatever_their_case() {
        let mut f = frontier();
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "/guide.PDF", 0), None);
        assert_eq!(f.offer(&base, "/logo.svg", 0), None);
        assert_eq!(f.offer(&base, "/about.html", 0), Some(1), "html is a page");
        assert_eq!(
            f.offer(&base, "/v1.2/notes", 0),
            Some(2),
            "a dot in a directory is not an extension"
        );
        assert_eq!(f.skipped().iter().filter(|(_, why)| *why == "file").count(), 2);
    }

    #[test]
    fn known_pages_are_not_queued_twice_and_report_their_index() {
        let mut f = frontier();
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "/a", 0), Some(1));
        assert_eq!(f.offer(&base, "/a#x", 0), Some(1));
        assert_eq!(f.offer(&base, "/", 0), Some(0), "the seed");
        let page = f.pop().unwrap();
        assert_eq!((page.index, page.depth), (0, 0));
        f.note_final(1, &url("http://site.test/landing"));
        assert_eq!(
            f.offer(&base, "/landing", 0),
            Some(1),
            "a redirect target is the same page"
        );
    }

    #[test]
    fn default_excludes_hold_back_logout_and_delete() {
        let mut f = frontier();
        let base = url("http://site.test/");
        for path in [
            "/logout",
            "/account/signout?next=/",
            "/log-out",
            "/items/delete",
            "/unsubscribe/abc",
            "/a/logout-all",
        ] {
            assert_eq!(f.offer(&base, path, 0), None, "{path}");
        }
        assert!(f.skipped().iter().all(|(_, why)| *why == "excluded"));
        assert_eq!(f.offer(&base, "/blog/login", 0), Some(1), "login is not logout");
    }

    #[test]
    fn include_globs_limit_the_crawl_to_matching_paths() {
        let mut cfg = config();
        cfg.include = vec!["/products/**".to_string()];
        let mut f = Frontier::new(
            url("http://site.test/"),
            cfg,
            Robots::allow_all(crate::robots::Status::NotFound),
        );
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "/about", 0), None);
        assert_eq!(f.skipped(), [("http://site.test/about".to_string(), "not-included")]);
        assert_eq!(f.offer(&base, "/products/1", 0), Some(1));
    }

    #[test]
    fn robots_rules_stop_a_link_before_it_is_queued() {
        let robots = Robots::parse("User-agent: *\nDisallow: /private\n", "traceqa");
        let mut f = Frontier::new(url("http://site.test/"), config(), robots);
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "/private/x", 0), None);
        assert_eq!(f.skipped()[0].1, "robots");
        assert_eq!(f.offer(&base, "/open", 0), Some(1));
    }

    #[test]
    fn depth_is_a_limit_and_too_deep_links_are_not_listed() {
        let mut cfg = config();
        cfg.depth = 1;
        let mut f = Frontier::new(
            url("http://site.test/"),
            cfg,
            Robots::allow_all(crate::robots::Status::NotFound),
        );
        let seed = url("http://site.test/");
        assert_eq!(f.offer(&seed, "/a", 0), Some(1));
        let a = url("http://site.test/a");
        assert_eq!(f.offer(&a, "/b", 1), None);
        assert!(f.skipped().is_empty());
        assert_eq!(f.offer(&a, "/", 1), Some(0), "a known page is still a link target");
    }

    #[test]
    fn max_pages_caps_the_map() {
        let mut cfg = config();
        cfg.max_pages = 3;
        let mut f = Frontier::new(
            url("http://site.test/"),
            cfg,
            Robots::allow_all(crate::robots::Status::NotFound),
        );
        let base = url("http://site.test/");
        assert_eq!(f.offer(&base, "/a", 0), Some(1));
        assert_eq!(f.offer(&base, "/b", 0), Some(2));
        assert_eq!(f.offer(&base, "/c", 0), None);
        assert_eq!(f.skipped(), [("http://site.test/c".to_string(), "max-pages")]);
    }

    #[test]
    fn numbers_and_hashes_collapse_into_one_pattern_with_a_cap() {
        assert_eq!(pattern_of(&url("http://s.test/products/12")), "/products/{n}");
        assert_eq!(pattern_of(&url("http://s.test/p/3f2a9c1d7b")), "/p/{h}");
        assert_eq!(
            pattern_of(&url("http://s.test/p/123e4567-e89b-12d3-a456-426614174000")),
            "/p/{h}"
        );
        assert_eq!(pattern_of(&url("http://s.test/about")), "/about");
        assert_eq!(
            pattern_of(&url("http://s.test/blog/deadbeef-post")),
            "/blog/deadbeef-post",
            "words are not hashes"
        );
        assert_eq!(pattern_of(&url("http://s.test/s?q=a&page=2")), "/s?page&q");
        let mut cfg = config();
        cfg.max_pages = 500;
        let mut f = Frontier::new(
            url("http://site.test/"),
            cfg,
            Robots::allow_all(crate::robots::Status::NotFound),
        );
        let base = url("http://site.test/");
        let found = (1..=30)
            .filter(|n| f.offer(&base, &format!("/item/{n}"), 0).is_some())
            .count();
        assert_eq!(found, PER_PATTERN);
        assert_eq!(
            f.skipped().iter().filter(|(_, why)| *why == "pattern-limit").count(),
            10
        );
    }

    #[test]
    fn skipped_addresses_carry_no_query_and_are_listed_once() {
        let mut f = frontier();
        let base = url("http://site.test/");
        f.offer(&base, "https://other.test/p?token=secret#frag", 0);
        f.offer(&base, "https://other.test/p?token=secret", 0);
        assert_eq!(f.skipped(), [("https://other.test/p".to_string(), "other-origin")]);
        f.offer(&base, "mailto:me@x.test?subject=hi", 0);
        assert_eq!(f.skipped()[1], ("mailto:me@x.test".to_string(), "scheme"));
    }

    #[test]
    fn globs_follow_shell_rules() {
        assert!(glob_matches("**/logout**", "/logout"));
        assert!(glob_matches("**/logout**", "/a/b/logout"));
        assert!(glob_matches("**/logout**", "/logout-all"));
        assert!(!glob_matches("**/logout**", "/blog"));
        assert!(glob_matches("/products/*", "/products/1"));
        assert!(!glob_matches("/products/*", "/products/1/reviews"));
        assert!(glob_matches("/products/**", "/products/1/reviews"));
        assert!(glob_matches("/a?c", "/abc"));
        assert!(!glob_matches("/a?c", "/a/c"));
        assert!(glob_matches("/docs", "/docs"));
        assert!(!glob_matches("/docs", "/docs/x"));
        assert!(glob_matches("**", "/anything/at/all"));
    }
}
