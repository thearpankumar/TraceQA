//! Patching a generated page object (`traceqa codegen`).
//!
//! Only one kind of line is ever changed: the `return` line of a property whose marker comment
//! (`traceqa:id c_...`) names a component, and only when that line still holds exactly the
//! expression that codegen wrote for the baseline. A line that a person edited is left alone and
//! reported. Patching twice gives the same text as patching once.

use traceqa_protocol::types::Locator;

/// The language of a page object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ts,
    Py,
}

impl Lang {
    /// From a file name: `.ts` or `.py`.
    pub fn of_path(path: &str) -> Option<Self> {
        if path.ends_with(".ts") {
            Some(Self::Ts)
        } else if path.ends_with(".py") {
            Some(Self::Py)
        } else {
            None
        }
    }
}

/// The expression of a locator as `traceqa codegen` writes it inside the class.
pub fn expression(locator: &Locator, lang: Lang) -> String {
    match lang {
        Lang::Ts => {
            let code = locator.playwright_ts();
            if let Some(rest) = code.strip_prefix("page.") {
                format!("this.page.{rest}")
            } else if let Some(rest) = code.strip_prefix("frameByLabel(page,") {
                format!("frameByLabel(this.page,{rest}")
            } else {
                code
            }
        }
        Lang::Py => {
            let code = locator.playwright_py();
            if let Some(rest) = code.strip_prefix("page.") {
                format!("self.page.{rest}")
            } else if let Some(rest) = code.strip_prefix("frame_by_label(page,") {
                format!("frame_by_label(self.page,{rest}")
            } else {
                code
            }
        }
    }
}

/// One property to rewrite.
#[derive(Debug, Clone)]
pub struct Edit {
    /// The id in the marker now.
    pub id: String,
    /// Expressions that count as "not edited by hand": what codegen wrote for the baseline.
    pub old: Vec<String>,
    /// The expression to write.
    pub new: String,
    /// When set, the marker is rewritten to this id (a rename that was accepted).
    pub new_id: Option<String>,
}

/// What [`patch`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patched {
    pub text: String,
    /// The ids whose line was changed.
    pub patched: Vec<String>,
    /// Ids that were left alone, each with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Is `line` a marker comment for exactly `id`?
fn is_marker(line: &str, id: &str) -> bool {
    let t = line.trim_start();
    if !(t.starts_with("//") || t.starts_with('#')) {
        return false;
    }
    let Some(at) = t.find("traceqa:id ") else { return false };
    let rest = t[at + "traceqa:id ".len()..].trim_end();
    rest == id
}

fn has_any_marker(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("//") || t.starts_with('#')) && t.contains("traceqa:id ")
}

/// Rewrites the properties named by `edits` in `source`.
pub fn patch(source: &str, lang: Lang, edits: &[Edit]) -> Patched {
    let mut lines: Vec<String> = source.split_inclusive('\n').map(str::to_string).collect();
    let mut patched = Vec::new();
    let mut skipped = Vec::new();
    for edit in edits {
        let Some(marker) = lines.iter().position(|l| is_marker(l, &edit.id)) else {
            skipped.push((edit.id.clone(), "no property is marked with this id".to_string()));
            continue;
        };
        let mut found = None;
        for (offset, line) in lines.iter().enumerate().skip(marker + 1) {
            if has_any_marker(line) {
                break;
            }
            if line.trim_start().starts_with("return ") {
                found = Some(offset);
                break;
            }
        }
        let Some(at) = found else {
            skipped.push((edit.id.clone(), "the property has no return line".to_string()));
            continue;
        };
        let line = lines[at].clone();
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let indent = &body[..body.len() - body.trim_start().len()];
        let current = body.trim_start()["return ".len()..].trim_end();
        let current = match lang {
            Lang::Ts => current.strip_suffix(';').unwrap_or(current),
            Lang::Py => current,
        };
        if current != edit.new && !edit.old.iter().any(|o| o == current) {
            skipped.push((
                edit.id.clone(),
                "the return line was edited by hand and is kept as it is".to_string(),
            ));
            continue;
        }
        let semicolon = if lang == Lang::Ts { ";" } else { "" };
        lines[at] = format!("{indent}return {}{semicolon}{ending}", edit.new);
        if let Some(new_id) = &edit.new_id {
            let marker_line = lines[marker].clone();
            if let Some(pos) = marker_line.rfind(&edit.id) {
                lines[marker] = format!(
                    "{}{}{}",
                    &marker_line[..pos],
                    new_id,
                    &marker_line[pos + edit.id.len()..]
                );
            }
        }
        patched.push(edit.id.clone());
    }
    Patched {
        text: lines.concat(),
        patched,
        skipped,
    }
}

/// A unified diff of two texts that have the same number of lines, as [`patch`] gives. Empty when
/// they are equal.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    const CONTEXT: usize = 2;
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let changed: Vec<usize> = (0..a.len().min(b.len())).filter(|&i| a[i] != b[i]).collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let (from, to) = (i.saturating_sub(CONTEXT), (i + CONTEXT + 1).min(a.len()));
        match hunks.last_mut() {
            Some(last) if from <= last.1 => last.1 = to,
            _ => hunks.push((from, to)),
        }
    }
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    for (from, to) in hunks {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            from + 1,
            to - from,
            from + 1,
            to - from
        ));
        for i in from..to {
            if a[i] == b[i] {
                out.push_str(&format!(" {}\n", a[i]));
            } else {
                out.push_str(&format!("-{}\n+{}\n", a[i], b[i]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "export class P {\n  // traceqa:id c_aaaaaaaaaaaa\n  // role: \"button\", name: \"Buy\"\n  get buy(): Locator {\n    return this.page.getByRole(\"button\", { name: \"Buy\", exact: true });\n  }\n\n  // traceqa:id c_bbbbbbbbbbbb\n  get other(): Locator {\n    return this.page.getByTestId(\"mine\"); // hand note\n  }\n}\n";

    fn edit(id: &str, old: &str, new: &str) -> Edit {
        Edit {
            id: id.into(),
            old: vec![old.into()],
            new: new.into(),
            new_id: None,
        }
    }

    #[test]
    fn only_the_return_line_under_the_marker_changes() {
        let old = "this.page.getByRole(\"button\", { name: \"Buy\", exact: true })";
        let out = patch(
            TS,
            Lang::Ts,
            &[edit("c_aaaaaaaaaaaa", old, "this.page.getByTestId(\"buy\")")],
        );
        assert_eq!(out.patched, ["c_aaaaaaaaaaaa"]);
        let before: Vec<&str> = TS.lines().collect();
        let after: Vec<&str> = out.text.lines().collect();
        let differing: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
        assert_eq!(differing, [4]);
        assert_eq!(after[4], "    return this.page.getByTestId(\"buy\");");
    }

    #[test]
    fn patching_twice_changes_nothing_more() {
        let old = "this.page.getByRole(\"button\", { name: \"Buy\", exact: true })";
        let e = [edit("c_aaaaaaaaaaaa", old, "this.page.getByTestId(\"buy\")")];
        let once = patch(TS, Lang::Ts, &e);
        let twice = patch(&once.text, Lang::Ts, &e);
        assert_eq!(once.text, twice.text);
    }

    #[test]
    fn a_hand_edited_line_is_kept() {
        let out = patch(
            TS,
            Lang::Ts,
            &[edit(
                "c_bbbbbbbbbbbb",
                "this.page.getByTestId(\"x\")",
                "this.page.getByTestId(\"y\")",
            )],
        );
        assert_eq!(out.text, TS);
        assert!(out.patched.is_empty());
        assert!(out.skipped[0].1.contains("by hand"));
    }

    #[test]
    fn a_missing_marker_is_reported() {
        let out = patch(TS, Lang::Ts, &[edit("c_cccccccccccc", "a", "b")]);
        assert_eq!(out.text, TS);
        assert!(out.skipped[0].1.contains("no property"));
    }

    #[test]
    fn a_prefix_of_an_id_is_not_the_id() {
        let out = patch(TS, Lang::Ts, &[edit("c_aaaaaaaaaaa", "a", "b")]);
        assert!(out.patched.is_empty());
    }

    #[test]
    fn an_accepted_rename_moves_the_marker_and_keeps_crlf() {
        let source = TS.replace('\n', "\r\n");
        let old = "this.page.getByRole(\"button\", { name: \"Buy\", exact: true })";
        let mut e = edit("c_aaaaaaaaaaaa", old, "this.page.getByTestId(\"buy\")");
        e.new_id = Some("c_dddddddddddd".into());
        let out = patch(&source, Lang::Ts, &[e]);
        assert!(out.text.contains("// traceqa:id c_dddddddddddd\r\n"));
        assert!(!out.text.contains("c_aaaaaaaaaaaa"));
        assert_eq!(out.text.matches("\r\n").count(), source.matches("\r\n").count());
    }

    #[test]
    fn python_returns_have_no_semicolon() {
        let py = "class P:\n    # traceqa:id c_aaaaaaaaaaaa\n    @property\n    def buy(self) -> Locator:\n        return self.page.get_by_role(\"button\", name=\"Buy\", exact=True)\n";
        let out = patch(
            py,
            Lang::Py,
            &[edit(
                "c_aaaaaaaaaaaa",
                "self.page.get_by_role(\"button\", name=\"Buy\", exact=True)",
                "self.page.get_by_test_id(\"buy\")",
            )],
        );
        assert!(out.text.ends_with("        return self.page.get_by_test_id(\"buy\")\n"));
    }

    #[test]
    fn the_diff_shows_the_changed_line_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nX\ne\nf\ng\nh\n";
        let diff = unified_diff("p.ts", old, new);
        assert!(diff.starts_with("--- a/p.ts\n+++ b/p.ts\n@@ -2,5 +2,5 @@\n"), "{diff}");
        assert!(diff.contains("-d\n+X\n"));
        assert_eq!(unified_diff("p.ts", old, old), "");
    }
}
