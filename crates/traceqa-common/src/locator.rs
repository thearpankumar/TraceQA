//! Renders a [`Locator`] as Playwright code, in TypeScript or Python.
//!
//! The engine describes how to find a component; this turns the description into the
//! expression a person would write. `traceqa codegen` and the MCP server's hints both use it,
//! so the two cannot drift apart.
//!
//! **Everything a locator holds is text from the page** (names, test ids, frame labels, CSS).
//! It reaches generated source only through [`ts_string`] and [`py_string`], which produce a
//! double-quoted literal that cannot end early and cannot contain a line break, so the same
//! literal is also safe at the end of a `//` or `#` comment.
//!
//! The generated code expects two helpers beside it, `frameByLabel(page, label)` (TypeScript)
//! and `frame_by_label(page, label)` (Python), for components inside an iframe.

use std::fmt::Write as _;

use crate::types::{Locator, LocatorStrategy};

/// Characters that are legal inside a string literal but invisible or able to reorder the text
/// around them (bidirectional controls, zero-width marks, line separators). They are written
/// as `\uXXXX` so that what a reader sees is what the program contains.
fn needs_escape(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'
        | '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
}

/// A double-quoted literal for `text`, valid in TypeScript and in Python alike.
///
/// Quotes and backslashes are escaped, and so are control characters, U+2028 and U+2029 and
/// invisible formatting characters. Other characters, Hindi and Arabic included, stay as they
/// are. `*/` is broken up so the literal is also safe inside a block comment.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut previous = '\0';
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '/' if previous == '*' => out.push_str("\\u002f"),
            c if needs_escape(c) => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
        previous = c;
    }
    out.push('"');
    out
}

/// `text` as a TypeScript string literal.
#[must_use]
pub fn ts_string(text: &str) -> String {
    quote(text)
}

/// `text` as a Python string literal.
#[must_use]
pub fn py_string(text: &str) -> String {
    quote(text)
}

/// The language a locator is rendered in.
#[derive(Clone, Copy)]
enum Lang {
    Ts,
    Py,
}

impl Lang {
    fn call(self, camel: &str) -> String {
        match self {
            Lang::Ts => camel.to_string(),
            Lang::Py => {
                let mut snake = String::new();
                for c in camel.chars() {
                    if c.is_ascii_uppercase() {
                        snake.push('_');
                        snake.push(c.to_ascii_lowercase());
                    } else {
                        snake.push(c);
                    }
                }
                snake
            }
        }
    }

    /// The options of a call, in the language's own syntax; empty when there are none.
    fn options(self, options: &[(&str, String)]) -> String {
        if options.is_empty() {
            return String::new();
        }
        match self {
            Lang::Ts => {
                let inner: Vec<String> = options.iter().map(|(key, value)| format!("{key}: {value}")).collect();
                format!(", {{ {} }}", inner.join(", "))
            }
            Lang::Py => {
                let inner: Vec<String> = options
                    .iter()
                    .map(|(key, value)| format!("{}={value}", self.call(key)))
                    .collect();
                format!(", {}", inner.join(", "))
            }
        }
    }

    fn literal(self, text: &str) -> String {
        match self {
            Lang::Ts => ts_string(text),
            Lang::Py => py_string(text),
        }
    }

    fn truth(self) -> String {
        match self {
            Lang::Ts => "true".to_string(),
            Lang::Py => "True".to_string(),
        }
    }
}

impl Locator {
    /// The locator as a TypeScript expression on a Playwright `page`.
    #[must_use]
    pub fn playwright_ts(&self) -> String {
        self.render(Lang::Ts)
    }

    /// The locator as a Python expression on a Playwright `page`.
    #[must_use]
    pub fn playwright_py(&self) -> String {
        self.render(Lang::Py)
    }

    fn render(&self, lang: Lang) -> String {
        let mut out = match &self.frame {
            Some(label) => match lang {
                Lang::Ts => format!("frameByLabel(page, {})", lang.literal(label)),
                Lang::Py => format!("frame_by_label(page, {})", lang.literal(label)),
            },
            None => "page".to_string(),
        };
        for scope in &self.scope {
            let mut options = Vec::new();
            if let Some(name) = &scope.name {
                options.push(("name", lang.literal(name)));
                options.push(("exact", lang.truth()));
            }
            let _ = write!(
                out,
                ".{}({}{})",
                lang.call("getByRole"),
                lang.literal(&scope.role),
                lang.options(&options)
            );
        }
        let mut options = Vec::new();
        match self.strategy {
            LocatorStrategy::Role => {
                if let Some(name) = &self.name {
                    options.push(("name", lang.literal(name)));
                    options.push(("exact", lang.truth()));
                }
                if self.needs_include_hidden {
                    options.push(("includeHidden", lang.truth()));
                }
                let role = self.role.as_deref().unwrap_or_default();
                let _ = write!(
                    out,
                    ".{}({}{})",
                    lang.call("getByRole"),
                    lang.literal(role),
                    lang.options(&options)
                );
            }
            LocatorStrategy::Testid => {
                let id = self.test_id.as_deref().unwrap_or_default();
                let _ = write!(out, ".{}({})", lang.call("getByTestId"), lang.literal(id));
            }
            LocatorStrategy::Text => {
                options.push(("exact", lang.truth()));
                let text = self.name.as_deref().unwrap_or_default();
                let _ = write!(
                    out,
                    ".{}({}{})",
                    lang.call("getByText"),
                    lang.literal(text),
                    lang.options(&options)
                );
            }
            LocatorStrategy::Css => {
                let css = self.css.as_deref().unwrap_or("*");
                let _ = write!(out, ".locator({})", lang.literal(css));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LocatorScope;

    fn role(name: Option<&str>) -> Locator {
        Locator {
            strategy: LocatorStrategy::Role,
            role: Some("button".into()),
            name: name.map(Into::into),
            exact: name.map(|_| true),
            test_id: None,
            scope: Vec::new(),
            frame: None,
            css: None,
            unique: true,
            matches: 1,
            own: 1,
            ambiguous_with: Vec::new(),
            forced: false,
            needs_include_hidden: false,
        }
    }

    #[test]
    fn a_role_and_name_in_both_languages() {
        let locator = role(Some("Pay now"));
        assert_eq!(
            locator.playwright_ts(),
            r#"page.getByRole("button", { name: "Pay now", exact: true })"#
        );
        assert_eq!(
            locator.playwright_py(),
            r#"page.get_by_role("button", name="Pay now", exact=True)"#
        );
    }

    #[test]
    fn scope_frame_and_hidden() {
        let mut locator = role(None);
        locator.frame = Some("pay".into());
        locator.needs_include_hidden = true;
        locator.scope = vec![
            LocatorScope {
                role: "dialog".into(),
                name: Some("Card".into()),
            },
            LocatorScope {
                role: "form".into(),
                name: None,
            },
        ];
        assert_eq!(
            locator.playwright_ts(),
            r#"frameByLabel(page, "pay").getByRole("dialog", { name: "Card", exact: true }).getByRole("form").getByRole("button", { includeHidden: true })"#
        );
        assert_eq!(
            locator.playwright_py(),
            r#"frame_by_label(page, "pay").get_by_role("dialog", name="Card", exact=True).get_by_role("form").get_by_role("button", include_hidden=True)"#
        );
    }

    #[test]
    fn the_other_strategies() {
        let mut locator = role(None);
        locator.strategy = LocatorStrategy::Testid;
        locator.test_id = Some("add".into());
        assert_eq!(locator.playwright_ts(), r#"page.getByTestId("add")"#);
        assert_eq!(locator.playwright_py(), r#"page.get_by_test_id("add")"#);
        locator.strategy = LocatorStrategy::Text;
        locator.name = Some("Gift cards".into());
        assert_eq!(
            locator.playwright_ts(),
            r#"page.getByText("Gift cards", { exact: true })"#
        );
        assert_eq!(locator.playwright_py(), r#"page.get_by_text("Gift cards", exact=True)"#);
        locator.strategy = LocatorStrategy::Css;
        locator.css = Some("#a > b".into());
        assert_eq!(locator.playwright_ts(), r##"page.locator("#a > b")"##);
        assert_eq!(locator.playwright_py(), r##"page.locator("#a > b")"##);
    }

    #[test]
    fn literals_escape_what_could_break_out() {
        assert_eq!(ts_string("plain"), "\"plain\"");
        assert_eq!(ts_string("say \"hi\""), r#""say \"hi\"""#);
        assert_eq!(ts_string("back\\slash"), r#""back\\slash""#);
        assert_eq!(ts_string("a\nb\r\tc"), r#""a\nb\r\tc""#);
        let bs = '\\';
        assert_eq!(ts_string("a\u{2028}b\u{2029}c"), format!("\"a{bs}u2028b{bs}u2029c\""));
        assert_eq!(ts_string("\u{0}\u{1b}"), r#""\u0000\u001b""#);
        // Bidirectional overrides would make the source read differently from what it does.
        assert_eq!(py_string("a\u{202e}b"), format!("\"a{bs}u202eb\""));
        // Text from other scripts stays readable.
        assert_eq!(ts_string("कार्ट देखें"), "\"कार्ट देखें\"");
        assert_eq!(ts_string("تواصل معنا"), "\"تواصل معنا\"");
        // A block comment cannot be closed by a name.
        assert_eq!(ts_string("a*/b"), format!("\"a*{bs}u002fb\""));
    }

    #[test]
    fn hostile_text_stays_inside_one_literal_on_one_line() {
        let hostile = [
            "\"); process.exit(1); //",
            "\\\"); import os; os.system('x') #",
            "line\nbreak\u{2028}here",
            "' + \"",
            "*/ evil /*",
            "\u{202e}rtl",
            "}); await page.close(); (() => {",
        ];
        for text in hostile {
            let literal = ts_string(text);
            assert!(literal.starts_with('"') && literal.ends_with('"'), "{literal}");
            let body: Vec<char> = literal.chars().skip(1).collect::<Vec<_>>();
            let body = &body[..body.len() - 1];
            // Every quote inside is escaped, and nothing is a line break.
            let mut escaped = false;
            for &c in body {
                assert!(!needs_escape(c), "raw control or format character in {literal}");
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else {
                    assert_ne!(c, '"', "unescaped quote in {literal}");
                }
            }
            assert!(!escaped, "dangling backslash in {literal}");
        }
    }
}
