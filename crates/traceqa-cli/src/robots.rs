//! robots.txt for `traceqa map`, after RFC 9309 (https://www.rfc-editor.org/rfc/rfc9309).
//!
//! * Rules are grouped by user-agent. The group for our product token `traceqa` is used when
//!   there is one, otherwise the `*` group; several groups with the same agent are one group.
//! * The longest matching pattern wins, and `Allow` wins a tie. `*` matches any run of
//!   characters and a final `$` anchors the end. Matching is against the path and the query.
//! * A 4xx answer means there is no robots.txt: everything is allowed. A 5xx answer or no
//!   answer at all means the file may be there but cannot be read: everything is disallowed.
//! * `Crawl-delay` is not part of the standard and is ignored; `--delay-ms` is our own.

use std::io::Read as _;
use std::time::Duration;

/// Our product token, matched case-insensitively against `User-agent` lines.
pub const AGENT: &str = "traceqa";

/// At most this much of the file is read (RFC 9309 asks parsers for at least 500 KiB).
const MAX_BYTES: u64 = 512 * 1024;

/// How the file was obtained. The strings are the `robots.status` values of `site.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The rules were read and apply.
    Fetched,
    /// A 4xx answer: no rules, everything allowed.
    NotFound,
    /// A 5xx answer or no answer: everything disallowed.
    Unavailable,
    /// `--no-robots`.
    Disabled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Fetched => "fetched",
            Status::NotFound => "not-found",
            Status::Unavailable => "unavailable",
            Status::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    allow: bool,
    pattern: String,
}

/// The rules that apply to us, and how they were obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Robots {
    rules: Vec<Rule>,
    disallow_all: bool,
    pub status: Status,
}

impl Robots {
    /// Everything is allowed, for `status`.
    pub fn allow_all(status: Status) -> Self {
        Robots {
            rules: Vec::new(),
            disallow_all: false,
            status,
        }
    }

    pub fn disallow_all() -> Self {
        Robots {
            rules: Vec::new(),
            disallow_all: true,
            status: Status::Unavailable,
        }
    }

    /// Reads a robots.txt body for the product token `agent`.
    pub fn parse(text: &str, agent: &str) -> Self {
        struct Group {
            agents: Vec<String>,
            rules: Vec<Rule>,
        }
        let mut groups: Vec<Group> = Vec::new();
        // True while the lines so far since the last rule were User-agent lines, so that the
        // next User-agent line joins the same group.
        let mut collecting_agents = false;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match key.as_str() {
                "user-agent" => {
                    if !collecting_agents || groups.is_empty() {
                        groups.push(Group {
                            agents: Vec::new(),
                            rules: Vec::new(),
                        });
                    }
                    if let Some(group) = groups.last_mut() {
                        group.agents.push(value.to_ascii_lowercase());
                    }
                    collecting_agents = true;
                }
                "allow" | "disallow" => {
                    collecting_agents = false;
                    // A rule before any User-agent line belongs to no group.
                    if let Some(group) = groups.last_mut() {
                        // An empty Disallow allows everything and an empty Allow says nothing.
                        if !value.is_empty() {
                            group.rules.push(Rule {
                                allow: key == "allow",
                                pattern: value.to_string(),
                            });
                        }
                    }
                }
                _ => collecting_agents = false,
            }
        }
        let agent = agent.to_ascii_lowercase();
        let named = |wanted: &str| -> Vec<Rule> {
            groups
                .iter()
                .filter(|g| g.agents.iter().any(|a| a == wanted))
                .flat_map(|g| g.rules.iter().cloned())
                .collect()
        };
        let has_named = groups.iter().any(|g| g.agents.contains(&agent));
        let rules = if has_named { named(&agent) } else { named("*") };
        Robots {
            rules,
            disallow_all: false,
            status: Status::Fetched,
        }
    }

    /// Whether `target` (a path with its query, starting with `/`) may be fetched.
    pub fn allowed(&self, target: &str) -> bool {
        if self.disallow_all {
            return false;
        }
        let mut best: Option<(usize, bool)> = None;
        for rule in &self.rules {
            if !pattern_matches(&rule.pattern, target) {
                continue;
            }
            let length = rule.pattern.len();
            best = match best {
                Some((len, allow)) if len > length || (len == length && allow) => Some((len, allow)),
                _ => Some((length, rule.allow)),
            };
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// `*` matches any run of characters (also none); a trailing `$` anchors the end; everything
/// else must match from the start of `target`.
fn pattern_matches(pattern: &str, target: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let pattern: Vec<char> = pattern.chars().collect();
    let target: Vec<char> = target.chars().collect();
    // Iterative wildcard matching with one backtrack point.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if p == pattern.len() {
            if !anchored || t == target.len() {
                return true;
            }
        } else if pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
            continue;
        } else if t < target.len() && pattern[p] == target[t] {
            p += 1;
            t += 1;
            continue;
        }
        match star {
            Some((star_p, star_t)) if star_t < target.len() => {
                star = Some((star_p, star_t + 1));
                p = star_p + 1;
                t = star_t + 1;
            }
            _ => return false,
        }
    }
}

/// Fetches and reads `origin`/robots.txt. Never fails: a file that cannot be read is
/// [`Robots::disallow_all`], as RFC 9309 asks.
pub fn fetch(origin: &str) -> Robots {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(format!("{AGENT}/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let agent: ureq::Agent = config.into();
    let address = format!("{}/robots.txt", origin.trim_end_matches('/'));
    let Ok(response) = agent.get(&address).call() else {
        return Robots::disallow_all();
    };
    let status = response.status().as_u16();
    match status {
        200..=299 => {
            let mut bytes = Vec::new();
            let read = response
                .into_body()
                .into_reader()
                .take(MAX_BYTES)
                .read_to_end(&mut bytes);
            match read {
                Ok(_) => Robots::parse(&String::from_utf8_lossy(&bytes), AGENT),
                Err(_) => Robots::disallow_all(),
            }
        }
        // Redirects past the limit and other odd answers: not a usable file, not a clear 4xx.
        400..=499 => Robots::allow_all(Status::NotFound),
        _ => Robots::disallow_all(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Robots {
        Robots::parse(text, AGENT)
    }

    #[test]
    fn disallow_blocks_a_prefix_and_everything_else_is_allowed() {
        let robots = parse("User-agent: *\nDisallow: /private\n");
        assert!(!robots.allowed("/private"));
        assert!(!robots.allowed("/private/page?x=1"));
        assert!(robots.allowed("/public"));
        assert!(robots.allowed("/"));
    }

    #[test]
    fn the_longest_match_wins_and_allow_wins_a_tie() {
        let robots = parse("User-agent: *\nDisallow: /shop\nAllow: /shop/sale\n");
        assert!(!robots.allowed("/shop/cart"));
        assert!(robots.allowed("/shop/sale/1"));
        let tie = parse("User-agent: *\nDisallow: /a\nAllow: /a\n");
        assert!(tie.allowed("/a"));
        let reversed = parse("User-agent: *\nAllow: /a\nDisallow: /a\n");
        assert!(reversed.allowed("/a"));
    }

    #[test]
    fn wildcards_and_the_end_anchor() {
        let robots = parse("User-agent: *\nDisallow: /*.pdf$\nDisallow: /a*/tmp\n");
        assert!(!robots.allowed("/docs/x.pdf"));
        assert!(
            robots.allowed("/docs/x.pdf?download=1"),
            "$ anchors the end of path and query"
        );
        assert!(robots.allowed("/docs/x.pdfx"));
        assert!(!robots.allowed("/abc/tmp"));
        assert!(!robots.allowed("/a/tmp/x"));
        assert!(robots.allowed("/b/tmp"));
    }

    #[test]
    fn the_named_group_beats_the_star_group_and_names_are_case_insensitive() {
        let text = "User-agent: *\nDisallow: /\n\nUser-agent: TraceQA\nDisallow: /secret\n";
        let robots = parse(text);
        assert!(robots.allowed("/open"));
        assert!(!robots.allowed("/secret/x"));
        let other = parse("User-agent: googlebot\nDisallow: /\n");
        assert!(
            other.allowed("/anything"),
            "a group for another agent says nothing about us"
        );
    }

    #[test]
    fn consecutive_user_agent_lines_share_their_rules() {
        let robots = parse("User-agent: bingbot\nUser-agent: traceqa\nDisallow: /x\n");
        assert!(!robots.allowed("/x"));
        assert!(robots.allowed("/y"));
    }

    #[test]
    fn an_empty_disallow_and_an_empty_file_allow_everything() {
        assert!(parse("User-agent: *\nDisallow:\n").allowed("/anything"));
        assert!(parse("").allowed("/anything"));
        assert!(parse("# only a comment\n").allowed("/anything"));
        assert!(parse("User-agent: *\n").allowed("/anything"));
    }

    #[test]
    fn comments_unknown_lines_and_crlf_are_ignored() {
        let robots = parse(
            "\u{feff}User-agent: *  # everyone\r\nCrawl-delay: 10\r\nSitemap: http://x/s.xml\r\nDisallow: /a # no\r\n",
        );
        assert!(!robots.allowed("/a"));
        assert!(robots.allowed("/b"));
    }

    #[test]
    fn disallow_all_blocks_everything_and_allow_all_nothing() {
        assert!(!Robots::disallow_all().allowed("/"));
        assert!(Robots::allow_all(Status::NotFound).allowed("/x"));
        assert_eq!(Robots::disallow_all().status, Status::Unavailable);
    }

    #[test]
    fn the_root_disallow_blocks_the_whole_site_but_allow_can_open_a_path() {
        let robots = parse("User-agent: *\nDisallow: /\nAllow: /public$\n");
        assert!(!robots.allowed("/"));
        assert!(robots.allowed("/public"));
        assert!(!robots.allowed("/public/x"));
    }
}
