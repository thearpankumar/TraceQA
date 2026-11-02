//! A `graph.json` read back, so that it can be replayed (backlog EXP-8).
//!
//! The file comes from outside: it may be old, edited by hand or made by something else. Nothing in it is trusted.
//! [`Doc::parse`] checks that it holds what a walk writes (names in order, every edge between screens that exist, an
//! action that worked leading somewhere) and gives a plain message for what it does not. Nothing here panics on
//! a bad file.

use std::collections::BTreeSet;

use serde_json::Value;
use thiserror::Error;

use super::graph::{Status, GRAPH_VERSION};
use crate::state::{Look, Screens, Similarity};

/// The most bytes of a graph that are read. A walk's graph is far smaller (10 000 screens and 100 000 actions at the
/// most, a few hundred bytes each).
pub const MAX_GRAPH_BYTES: u64 = 64 * 1024 * 1024;

/// The most screens and edges a graph may hold: the largest values `explore` accepts for `--max-states` and
/// `--max-actions`, with room for the edges that a walk makes beyond its actions (those that found nothing to act on).
const MAX_STATES: usize = 10_000;
const MAX_EDGES: usize = 200_000;

/// The longest text an edge may carry to type. A walk keeps answers to 200 characters; this is a bound on what a
/// file may hold, not a promise of what is typed.
const MAX_VALUE_CHARS: usize = 1_000;

/// Why a file is not a graph that can be replayed. The message names the file's own words, never its contents.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct DocError(pub String);

fn bad<T>(message: impl Into<String>) -> Result<T, DocError> {
    Err(DocError(message.into()))
}

/// What an action does to its component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Click,
    Type,
    /// A click that was blocked and was done with a key.
    Press,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Type => "type",
            Self::Press => "press",
        }
    }
}

/// One screen of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocState {
    pub id: String,
    /// The cleaned address, with secret parameters hidden.
    pub url: String,
    pub modal: Option<String>,
    pub components: Vec<String>,
    pub frames: Vec<String>,
}

/// One action of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEdge {
    pub id: String,
    /// The position of the screen it was made on.
    pub from: usize,
    pub component: String,
    pub label: String,
    pub action: ActionKind,
    /// The text that was typed. Only on a `type` edge of a graph of version 2 or later.
    pub value: Option<String>,
    /// The position of the screen it led to. Always there when `status` is ok.
    pub to: Option<usize>,
    pub status: Status,
}

/// What a replay needs of a `graph.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// The version of the layout the file was written in.
    pub version: u32,
    pub start_url: String,
    pub similarity: Similarity,
    /// The ids that are on (nearly) every page.
    pub chrome: BTreeSet<String>,
    pub states: Vec<DocState>,
    pub edges: Vec<DocEdge>,
}

impl Doc {
    /// Reads and checks a graph.
    pub fn parse(text: &str) -> Result<Self, DocError> {
        let root: Value = serde_json::from_str(text).map_err(|error| DocError(format!("it is not JSON ({error})")))?;
        if !root.is_object() {
            return bad("it is not a graph: the top level is not an object");
        }
        let version = match root["schemaVersion"].as_u64() {
            Some(version) if (1..=u64::from(GRAPH_VERSION)).contains(&version) => version as u32,
            Some(version) if version > u64::from(GRAPH_VERSION) => {
                return bad(format!(
                    "it was written in graph layout {version}, which is newer than this traceqa knows ({GRAPH_VERSION})"
                ))
            }
            _ => return bad("it has no schemaVersion: it is not a graph written by `traceqa explore`"),
        };
        let start_url = text_of(&root, "startUrl")?.to_string();
        if !is_web_address(&start_url) {
            return bad("startUrl is not an http or https address");
        }
        let per_mille = match root["stateSimilarityPerMille"].as_u64() {
            Some(n) if n <= 1000 => n,
            _ => return bad("stateSimilarityPerMille is not a number from 0 to 1000"),
        };
        let similarity = Similarity::new(per_mille as f64 / 1000.0).map_err(|error| DocError(error.to_string()))?;
        let chrome = strings_of(&root, "chrome")?.into_iter().collect();

        let states = list_of(&root, "states", MAX_STATES)?;
        if states.is_empty() {
            return bad("it has no screens");
        }
        let states = states
            .iter()
            .enumerate()
            .map(|(position, state)| read_state(position, state))
            .collect::<Result<Vec<_>, _>>()?;
        let edges = list_of(&root, "edges", MAX_EDGES)?
            .iter()
            .enumerate()
            .map(|(position, edge)| read_edge(position, edge, states.len()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            version,
            start_url,
            similarity,
            chrome,
            states,
            edges,
        })
    }

    /// The screens of the graph, named as the walk named them, to tell whether a page is one of them by the same rule
    /// the walk used. The graph holds the ids a screen is compared by, so no reading of the pages is needed.
    pub fn screens(&self) -> Result<Screens, DocError> {
        let mut screens = Screens::new(self.similarity, self.chrome.clone());
        for (position, state) in self.states.iter().enumerate() {
            let mut look = Look::new(&state.url, state.components.iter().cloned(), state.modal.as_deref());
            look.frames = state.frames.iter().cloned().collect();
            match screens.observe(&look) {
                Ok(seen) if seen.is_new && seen.id.index() == position => {}
                Ok(_) => {
                    return bad(format!(
                        "{} is alike an earlier screen under the graph's own similarity, which a walk never writes",
                        state.id
                    ))
                }
                Err(error) => return bad(format!("{} cannot be used: {error}", state.id)),
            }
        }
        Ok(screens)
    }

    /// Says that the site is now at `origin`: every address of the graph that begins with the origin the walk started
    /// at is changed to begin with this one. For a site that is served on another port than when it was explored.
    pub fn rebase(&mut self, origin: &str) -> Result<(), DocError> {
        let Some(to) = origin_of(origin) else {
            return bad(format!("{origin:?} is not an http or https address"));
        };
        let Some(from) = origin_of(&self.start_url) else {
            return bad("startUrl is not an http or https address");
        };
        let change = |url: &mut String| {
            if let Some(rest) = url.strip_prefix(&from) {
                // Only whole origins: the port 4173 is not the start of 41730.
                if rest.is_empty() || rest.starts_with(['/', '?', '#']) {
                    *url = format!("{to}{rest}");
                }
            }
        };
        change(&mut self.start_url);
        for state in &mut self.states {
            change(&mut state.url);
        }
        Ok(())
    }

    /// Whether the start address has parameters that were hidden when the graph was written, so that it cannot be
    /// opened again.
    pub fn start_is_hidden(&self) -> bool {
        self.start_url.contains(crate::url_safety::REDACTED)
    }

    /// The position of a screen named `sN`, when the graph has it.
    pub fn state_index(&self, name: &str) -> Option<usize> {
        index_of(name, 's').filter(|&position| position < self.states.len())
    }

    /// The position of an edge named `eN`, when the graph has it.
    pub fn edge_index(&self, name: &str) -> Option<usize> {
        index_of(name, 'e').filter(|&position| position < self.edges.len())
    }
}

fn read_state(position: usize, state: &Value) -> Result<DocState, DocError> {
    let id = text_of(state, "id")?;
    if id != format!("s{position}") {
        return bad(format!("screen number {position} is called {id:?}, not s{position}"));
    }
    let url = text_of(state, "url")?.to_string();
    if !is_web_address(&url) {
        return bad(format!("{id} has an address that is not http or https"));
    }
    let components = strings_of(state, "components")?;
    if components.is_empty() {
        return bad(format!("{id} has no components"));
    }
    let modal = match state.get("modal") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => return bad(format!("{id} has a modal that is not text")),
    };
    Ok(DocState {
        id: id.to_string(),
        url,
        modal,
        components,
        frames: strings_of(state, "frames")?,
    })
}

fn read_edge(position: usize, edge: &Value, states: usize) -> Result<DocEdge, DocError> {
    let id = text_of(edge, "id")?;
    if id != format!("e{position}") {
        return bad(format!("edge number {position} is called {id:?}, not e{position}"));
    }
    let screen = |key: &str| -> Result<usize, DocError> {
        let name = text_of(edge, key)?;
        match index_of(name, 's') {
            Some(at) if at < states => Ok(at),
            _ => bad(format!("{id} has {key} {name:?}, which is not a screen of the graph")),
        }
    };
    let from = screen("from")?;
    let component = text_of(edge, "component")?;
    if component.is_empty() {
        return bad(format!("{id} has no component"));
    }
    let action = match text_of(edge, "action")? {
        "click" => ActionKind::Click,
        "type" => ActionKind::Type,
        "press" => ActionKind::Press,
        other => return bad(format!("{id} has an action {other:?} that is not click, type or press")),
    };
    let status = match text_of(edge, "status")? {
        "ok" => Status::Ok,
        "missing" => Status::Missing,
        "timeout" => Status::Timeout,
        "failed" => Status::Failed,
        other => {
            return bad(format!(
                "{id} has a status {other:?} that is not ok, missing, timeout or failed"
            ))
        }
    };
    let to = match edge.get("to") {
        None | Some(Value::Null) => None,
        Some(_) => Some(screen("to")?),
    };
    if status == Status::Ok && to.is_none() {
        return bad(format!("{id} worked and leads nowhere"));
    }
    let value = match (action, edge.get("value")) {
        (ActionKind::Type, Some(Value::String(text))) => {
            if text.chars().count() > MAX_VALUE_CHARS {
                return bad(format!("{id} has a value longer than {MAX_VALUE_CHARS} characters"));
            }
            Some(text.clone())
        }
        (_, None | Some(Value::Null)) | (ActionKind::Click | ActionKind::Press, Some(_)) => None,
        (ActionKind::Type, Some(_)) => return bad(format!("{id} has a value that is not text")),
    };
    Ok(DocEdge {
        id: id.to_string(),
        from,
        component: component.to_string(),
        label: edge
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        action,
        value,
        to,
        status,
    })
}

fn text_of<'v>(object: &'v Value, key: &str) -> Result<&'v str, DocError> {
    match object.get(key).and_then(Value::as_str) {
        Some(text) => Ok(text),
        None => bad(format!("{key} is missing or is not text")),
    }
}

fn list_of<'v>(object: &'v Value, key: &str, most: usize) -> Result<&'v Vec<Value>, DocError> {
    match object.get(key).and_then(Value::as_array) {
        Some(list) if list.len() <= most => Ok(list),
        Some(_) => bad(format!("{key} has more than {most} entries")),
        None => bad(format!("{key} is missing or is not a list")),
    }
}

fn strings_of(object: &Value, key: &str) -> Result<Vec<String>, DocError> {
    list_of(object, key, MAX_EDGES)?
        .iter()
        .map(|item| match item.as_str() {
            Some(text) => Ok(text.to_string()),
            None => bad(format!("{key} holds something that is not text")),
        })
        .collect()
}

/// `s12` is 12 for the prefix `s`, and nothing else is anything: no sign, no space, no leading zero.
fn index_of(name: &str, prefix: char) -> Option<usize> {
    let digits = name.strip_prefix(prefix)?;
    let plain =
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && (digits == "0" || !digits.starts_with('0'));
    plain.then(|| digits.parse().ok()).flatten()
}

fn is_web_address(address: &str) -> bool {
    let lower = address.get(..8).unwrap_or(address).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `scheme://host:port` of an http or https address.
fn origin_of(address: &str) -> Option<String> {
    let url = url::Url::parse(address).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph() -> Value {
        json!({
            "schemaVersion": GRAPH_VERSION,
            "startUrl": "http://127.0.0.1:4173/shop/",
            "stateSimilarityPerMille": 900,
            "chrome": ["c_head"],
            "states": [
                { "id": "s0", "url": "http://127.0.0.1:4173/shop/", "components": ["c_a", "c_b"], "frames": [], "depth": 0 },
                { "id": "s1", "url": "http://127.0.0.1:4173/shop/cart.html", "components": ["c_c"], "frames": [], "depth": 1 },
            ],
            "edges": [
                { "id": "e0", "from": "s0", "component": "c_a", "label": "link \"Cart\"", "action": "click",
                  "status": "ok", "to": "s1", "navigated": true },
                { "id": "e1", "from": "s0", "component": "c_b", "label": "textbox \"City\"", "action": "type",
                  "value": "Pune", "status": "ok", "to": "s0" },
            ],
        })
    }

    fn parse(value: &Value) -> Result<Doc, DocError> {
        Doc::parse(&value.to_string())
    }

    fn refused(value: &Value) -> String {
        parse(value).expect_err("the file should be refused").0
    }

    #[test]
    fn a_graph_a_walk_wrote_is_read() {
        let doc = parse(&graph()).unwrap();
        assert_eq!(doc.states.len(), 2);
        assert_eq!(doc.edges[0].to, Some(1));
        assert_eq!(doc.edges[1].value.as_deref(), Some("Pune"));
        assert_eq!(doc.edges[0].value, None);
        assert_eq!(doc.similarity.per_mille(), 900);
        assert_eq!(doc.state_index("s1"), Some(1));
        assert_eq!(doc.state_index("s2"), None);
        assert_eq!(doc.edge_index("e1"), Some(1));
    }

    #[test]
    fn the_screens_of_a_graph_are_named_as_the_walk_named_them() {
        let doc = parse(&graph()).unwrap();
        let screens = doc.screens().unwrap();
        assert_eq!(screens.len(), 2);
        assert_eq!(screens.all()[1].id.to_string(), "s1");
        let look = Look::new("http://127.0.0.1:4173/shop/cart.html", ["c_c", "c_head"], None);
        assert!(
            screens.matches(screens.all()[1].id, &look),
            "the page-wide id is left out"
        );
        let other = Look::new("http://127.0.0.1:4173/shop/cart.html", ["c_x"], None);
        assert!(!screens.matches(screens.all()[1].id, &other));
    }

    #[test]
    fn text_that_is_not_json_or_not_an_object_is_refused() {
        assert!(Doc::parse("{ nope").is_err());
        assert!(Doc::parse("[]").is_err());
        assert!(Doc::parse("").is_err());
    }

    #[test]
    fn a_graph_from_a_newer_traceqa_or_with_no_version_is_refused_by_name() {
        let mut newer = graph();
        newer["schemaVersion"] = json!(GRAPH_VERSION + 1);
        assert!(refused(&newer).contains("newer"));
        let mut none = graph();
        none.as_object_mut().unwrap().remove("schemaVersion");
        assert!(refused(&none).contains("schemaVersion"));
        let mut zero = graph();
        zero["schemaVersion"] = json!(0);
        assert!(parse(&zero).is_err());
    }

    #[test]
    fn a_graph_of_the_first_layout_is_read_and_its_typed_edges_have_no_value() {
        let mut old = graph();
        old["schemaVersion"] = json!(1);
        old["edges"][1].as_object_mut().unwrap().remove("value");
        let doc = parse(&old).unwrap();
        assert_eq!(doc.edges[1].action, ActionKind::Type);
        assert_eq!(doc.edges[1].value, None);
    }

    #[test]
    fn names_out_of_order_or_unknown_are_refused() {
        let mut state = graph();
        state["states"][1]["id"] = json!("s5");
        assert!(refused(&state).contains("s5"));
        let mut edge = graph();
        edge["edges"][1]["id"] = json!("e7");
        assert!(refused(&edge).contains("e7"));
        let mut from = graph();
        from["edges"][0]["from"] = json!("s9");
        assert!(refused(&from).contains("s9"));
        let mut to = graph();
        to["edges"][0]["to"] = json!("s01");
        assert!(refused(&to).contains("s01"));
        let mut sign = graph();
        sign["edges"][0]["from"] = json!("s+1");
        assert!(parse(&sign).is_err());
    }

    #[test]
    fn an_action_that_worked_and_leads_nowhere_is_refused_but_a_failed_one_may_lead_nowhere() {
        let mut nowhere = graph();
        nowhere["edges"][0].as_object_mut().unwrap().remove("to");
        assert!(refused(&nowhere).contains("leads nowhere"));
        let mut failed = graph();
        failed["edges"][0].as_object_mut().unwrap().remove("to");
        failed["edges"][0]["status"] = json!("failed");
        assert_eq!(parse(&failed).unwrap().edges[0].status, Status::Failed);
    }

    #[test]
    fn words_that_are_not_in_the_layout_are_refused() {
        let mut action = graph();
        action["edges"][0]["action"] = json!("drag");
        assert!(refused(&action).contains("drag"));
        let mut status = graph();
        status["edges"][0]["status"] = json!("great");
        assert!(refused(&status).contains("great"));
    }

    #[test]
    fn addresses_that_are_not_web_addresses_are_refused() {
        for bad_address in ["file:///etc/passwd", "javascript:alert(1)", "ftp://x/", "", "/shop/"] {
            let mut start = graph();
            start["startUrl"] = json!(bad_address);
            assert!(parse(&start).is_err(), "{bad_address:?} as the start address");
            let mut state = graph();
            state["states"][1]["url"] = json!(bad_address);
            assert!(parse(&state).is_err(), "{bad_address:?} as a screen address");
        }
    }

    #[test]
    fn missing_or_wrong_typed_fields_are_refused_and_never_panic() {
        for key in ["startUrl", "stateSimilarityPerMille", "chrome", "states", "edges"] {
            let mut file = graph();
            file.as_object_mut().unwrap().remove(key);
            assert!(parse(&file).is_err(), "without {key}");
        }
        let mut ids = graph();
        ids["states"][0]["components"] = json!([1, 2]);
        assert!(parse(&ids).is_err());
        let mut empty = graph();
        empty["states"][0]["components"] = json!([]);
        assert!(refused(&empty).contains("no components"));
        let mut none = graph();
        none["states"] = json!([]);
        assert!(refused(&none).contains("no screens"));
        let mut similarity = graph();
        similarity["stateSimilarityPerMille"] = json!(1001);
        assert!(parse(&similarity).is_err());
        let mut modal = graph();
        modal["states"][0]["modal"] = json!(5);
        assert!(parse(&modal).is_err());
        let mut component = graph();
        component["edges"][0]["component"] = json!("");
        assert!(parse(&component).is_err());
    }

    #[test]
    fn a_value_is_kept_only_for_a_typed_edge_and_is_bounded() {
        let mut click = graph();
        click["edges"][0]["value"] = json!("ignored");
        assert_eq!(parse(&click).unwrap().edges[0].value, None);
        let mut long = graph();
        long["edges"][1]["value"] = json!("x".repeat(MAX_VALUE_CHARS + 1));
        assert!(refused(&long).contains("longer"));
        let mut number = graph();
        number["edges"][1]["value"] = json!(5);
        assert!(refused(&number).contains("not text"));
        let mut exact = graph();
        exact["edges"][1]["value"] = json!("é".repeat(MAX_VALUE_CHARS));
        assert!(parse(&exact).is_ok(), "the bound counts characters and not bytes");
    }

    #[test]
    fn a_graph_with_too_many_screens_or_edges_is_refused() {
        let mut states = graph();
        states["states"] = json!(vec![json!({}); MAX_STATES + 1]);
        assert!(refused(&states).contains("more than"));
        let mut edges = graph();
        edges["edges"] = json!(vec![json!({}); MAX_EDGES + 1]);
        assert!(refused(&edges).contains("more than"));
    }

    #[test]
    fn two_screens_that_are_alike_are_not_a_graph_a_walk_wrote() {
        let mut twins = graph();
        twins["states"][1]["url"] = json!("http://127.0.0.1:4173/shop/");
        twins["states"][1]["components"] = json!(["c_a", "c_b"]);
        let doc = parse(&twins).unwrap();
        assert!(doc.screens().err().unwrap().0.contains("alike"));
    }

    #[test]
    fn rebasing_changes_the_origin_and_only_the_whole_origin() {
        let mut doc = parse(&graph()).unwrap();
        doc.states[1].url = "http://127.0.0.1:41730/shop/cart.html".to_string();
        doc.states[0].url = "http://127.0.0.1:4173".to_string();
        doc.rebase("http://127.0.0.1:5555/anything?x=1").unwrap();
        assert_eq!(doc.start_url, "http://127.0.0.1:5555/shop/");
        assert_eq!(doc.states[0].url, "http://127.0.0.1:5555");
        assert_eq!(
            doc.states[1].url, "http://127.0.0.1:41730/shop/cart.html",
            "port 41730 is not port 4173"
        );
        assert!(doc.rebase("ftp://x/").is_err());
        assert!(doc.rebase("not an address").is_err());
    }
    #[test]
    fn the_limits_are_the_ones_written_down() {
        assert_eq!(MAX_GRAPH_BYTES, 67_108_864);
    }

    #[test]
    fn a_version_of_zero_is_not_called_newer_and_a_similarity_over_a_thousand_is_refused() {
        let mut zero = graph();
        zero["schemaVersion"] = json!(0);
        let message = refused(&zero);
        assert!(
            message.contains("no schemaVersion") && !message.contains("newer"),
            "{message}"
        );
        let mut over = graph();
        over["stateSimilarityPerMille"] = json!(1001);
        assert!(refused(&over).contains("stateSimilarityPerMille"));
        let mut top = graph();
        top["stateSimilarityPerMille"] = json!(1000);
        assert!(parse(&top).is_ok());
    }

    #[test]
    fn an_address_with_a_hidden_part_is_told_apart() {
        let mut doc = parse(&graph()).unwrap();
        assert!(!doc.start_is_hidden());
        doc.start_url = format!("http://127.0.0.1:4173/?token={}", crate::url_safety::REDACTED);
        assert!(doc.start_is_hidden());
    }

    #[test]
    fn a_name_one_past_the_end_is_not_a_screen_or_an_edge() {
        let doc = parse(&graph()).unwrap();
        assert_eq!(doc.state_index("s1"), Some(1));
        assert_eq!(doc.state_index("s2"), None);
        assert_eq!(doc.edge_index("e1"), Some(1));
        assert_eq!(doc.edge_index("e2"), None);
    }

    #[test]
    fn an_edge_from_a_screen_one_past_the_end_is_refused() {
        let mut from = graph();
        from["edges"][0]["from"] = json!("s2");
        assert!(refused(&from).contains("not a screen"));
        let mut to = graph();
        to["edges"][0]["to"] = json!("s2");
        assert!(refused(&to).contains("not a screen"));
    }
}
