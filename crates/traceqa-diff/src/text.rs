//! Making text from a page safe to put in a Markdown report.
//!
//! Component names, paths and roles come from the page, so they are untrusted: a page can name a
//! button with backticks, pipes, newlines, right-to-left marks, a mention or HTML. Every such
//! string goes through [`code_span`], which is the only way page text reaches the Markdown.

/// Characters that show nothing or that reorder what is around them: control characters (a tab
/// and a newline stay), soft hyphen, Arabic format marks, zero-width and bidi characters, the
/// invisible operators, the byte order mark, the Unicode Tags block that hides ASCII text.
/// The same table as `crates/traceqa-mcp/src/tools.rs`.
fn is_invisible(c: char) -> bool {
    if c.is_control() {
        return c != '\n' && c != '\t';
    }
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x08E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB
        | 0x110BD | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE01EF)
}

/// The longest a name, path or role is shown, in characters.
pub const MAX_CHARS: usize = 80;

/// Removes invisible characters and turns every line break or tab into a space.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !is_invisible(*c))
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect()
}

/// Cleans `text`, cuts it to [`MAX_CHARS`] characters and returns it as a Markdown code span that
/// cannot end early: the fence is one backtick longer than the longest run inside. A pipe becomes
/// U+2223 (a look-alike), because a pipe would split a table cell whatever its escaping.
/// Text with nothing left to show gives `(empty)`.
pub fn code_span(text: &str) -> String {
    let mut shown = clean(text).replace('|', "\u{2223}");
    if shown.trim().is_empty() {
        return "(empty)".to_string();
    }
    if shown.chars().count() > MAX_CHARS {
        shown = shown.chars().take(MAX_CHARS - 1).collect::<String>() + "\u{2026}";
    }
    let longest = longest_backtick_run(&shown);
    let fence = "`".repeat(longest + 1);
    let pad = if shown.starts_with('`') || shown.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{shown}{pad}{fence}")
}

fn longest_backtick_run(text: &str) -> usize {
    let (mut longest, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_gets_one_backtick() {
        assert_eq!(code_span("Add to cart"), "`Add to cart`");
    }

    #[test]
    fn the_fence_is_longer_than_any_backtick_run() {
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("a```b"), "````a```b````");
        assert_eq!(code_span("`x`"), "`` `x` ``");
    }

    #[test]
    fn invisible_and_breaking_characters_go() {
        let shown = code_span("a\u{202E}b\u{200B}c\nd\te\u{E0041}\u{0007}");
        assert_eq!(shown, "`abc d e`");
    }

    #[test]
    fn a_pipe_does_not_survive() {
        assert!(!code_span("a|b").contains('|'));
    }

    #[test]
    fn long_text_is_cut_to_eighty_characters() {
        let shown = code_span(&"x".repeat(500));
        assert_eq!(shown.chars().count(), MAX_CHARS + 2);
        assert!(shown.ends_with("\u{2026}`"));
        let multibyte = code_span(&"\u{e9}".repeat(500));
        assert_eq!(multibyte.chars().count(), MAX_CHARS + 2);
    }

    #[test]
    fn nothing_visible_is_marked_empty() {
        assert_eq!(code_span(""), "(empty)");
        assert_eq!(code_span("\u{200B} \n"), "(empty)");
    }
}
