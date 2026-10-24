//! Keeps secrets out of what TraceQA writes down.
//!
//! The browser must load the address exactly as the person gave it, but nothing TraceQA
//! saves or reports (run folders, events, manifests, warnings) should hold a token that
//! was part of that address. [`check_navigable`] refuses addresses that carry a user name
//! or password, and [`redact`] hides the values of well-known secret parameters.

use url::Url;

/// Names of query and fragment parameters whose values are secrets. A name is compared
/// after percent-decoding, in lower case, with `-` read as `_`. Listed in `spec/README.md`.
pub const SECRET_KEYS: [&str; 15] = [
    "token",
    "access_token",
    "id_token",
    "refresh_token",
    "api_key",
    "apikey",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "sig",
    "signature",
    "x_amz_signature",
    "x_amz_credential",
    "x_amz_security_token",
];

/// What a hidden value is replaced with.
pub const REDACTED: &str = "REDACTED";

const USERINFO_HINT: &str = "the address contains a user name or password; put the sign-in in a state file instead: traceqa login ... then --storage-state";

fn scheme_error(address: &str) -> String {
    format!("url must start with http:// or https://, got {address:?}")
}

/// Accepts only addresses a browser may be sent to: `http` or `https`, with no user name or
/// password. The address is parsed the way browsers parse it (WHATWG), so forms that hide
/// a login from a plain text check (a tab or newline inside it) are caught too.
pub fn check_navigable(address: &str) -> Result<(), String> {
    let parsed = match Url::parse(address) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => parsed,
        Ok(_) => return Err(scheme_error(address)),
        Err(error) => {
            let lower = address.trim_start().to_ascii_lowercase();
            return Err(if lower.starts_with("http://") || lower.starts_with("https://") {
                format!("not a valid address ({error}): {address:?}")
            } else {
                scheme_error(address)
            });
        }
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(USERINFO_HINT.to_string());
    }
    // Browsers read a backslash before the path as `/`, other tools do not, so what the
    // browser loads and what a check elsewhere sees can differ. Refuse the doubt.
    let after_scheme = address.find("://").map_or(0, |at| at + 3);
    let authority_end = address[after_scheme..]
        .find(['/', '?', '#'])
        .map_or(address.len(), |at| after_scheme + at);
    if address[after_scheme..authority_end].contains('\\') {
        return Err("the address has a backslash before its path; write it with forward slashes".to_string());
    }
    Ok(())
}

/// The address with the value of every secret parameter in its query and fragment replaced
/// by [`REDACTED`], and any user name or password dropped. The text is edited in place, not
/// parsed and written again, so an address with nothing to hide comes back byte for byte.
pub fn redact(address: &str) -> String {
    let (before_fragment, fragment) = match address.split_once('#') {
        Some((head, fragment)) => (head, Some(fragment)),
        None => (address, None),
    };
    let (base, query) = match before_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (before_fragment, None),
    };
    let mut out = strip_userinfo(base);
    if let Some(query) = query {
        out.push('?');
        out.push_str(&redact_pairs(query));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(&redact_pairs(fragment));
    }
    out
}

/// `text` with every `http://` or `https://` address in it passed through [`redact`]. For text
/// that comes from a driver (error messages, log lines): Playwright quotes the address it failed
/// to open, and a query can carry a token. An address ends at white space or one of `"'<>)`. Text
/// without an address comes back unchanged.
pub fn redact_text(text: &str) -> String {
    // ASCII lower case keeps every byte position, so positions found here fit `text`.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(found) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| lower[from..].find(scheme))
        .min()
    {
        let start = from + found;
        let end = text[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')'))
            .map_or(text.len(), |at| start + at);
        out.push_str(&text[from..start]);
        out.push_str(&redact(&text[start..end]));
        from = end;
    }
    out.push_str(&text[from..]);
    out
}

fn strip_userinfo(base: &str) -> String {
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let end = rest.find(['/', '\\']).unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => base.to_string(),
    }
}

fn redact_pairs(text: &str) -> String {
    text.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_secret_key(key) => format!("{key}={REDACTED}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_secret_key(key: &str) -> bool {
    let key = percent_decode(key).to_lowercase().replace('-', "_");
    SECRET_KEYS.contains(&key.as_str())
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_urls_are_refused() {
        for address in [
            "https://u:p@h/",
            "https://u@h/",
            "http://:p@h/",
            "https://h\\@evil/",
            "https://u:p\t@h/",
            "https://u\n:p@h/",
            "https://h\t\n:80\\@evil/",
        ] {
            assert!(check_navigable(address).is_err(), "{address:?} should be refused");
        }
        assert!(check_navigable("https://u:p@h/")
            .unwrap_err()
            .contains("--storage-state"));
    }

    #[test]
    fn plain_addresses_pass() {
        for address in [
            "https://shop.test",
            "http://localhost:8080/a?b=c#d",
            "https://h/path@with-at",
            "https://h/?next=a\\b",
            "https://h?a=b@c",
            "HTTPS://Shop.Test/",
        ] {
            assert!(check_navigable(address).is_ok(), "{address}");
        }
    }

    #[test]
    fn scheme_gate_keeps_refusing_file_javascript_data() {
        for address in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://h/",
            "h.test",
            "",
        ] {
            let error = check_navigable(address).unwrap_err();
            assert!(error.contains("http:// or https://"), "{address}: {error}");
        }
    }

    #[test]
    fn redact_hides_each_secret_key_case_insensitively_in_query_and_fragment() {
        for key in SECRET_KEYS {
            for spelled in [key.to_string(), key.to_uppercase(), key.replace('_', "-")] {
                assert_eq!(
                    redact(&format!("https://h/p?{spelled}=abc&z=1")),
                    format!("https://h/p?{spelled}=REDACTED&z=1")
                );
                assert_eq!(
                    redact(&format!("https://h/p#{spelled}=abc&z=1")),
                    format!("https://h/p#{spelled}=REDACTED&z=1")
                );
            }
        }
        assert_eq!(
            redact("https://h/cb?code=1#access_token=abc&token_type=bearer"),
            "https://h/cb?code=1#access_token=REDACTED&token_type=bearer"
        );
    }

    #[test]
    fn redact_keeps_other_params_and_returns_unchanged_urls_byte_for_byte() {
        for address in [
            "https://shop.test",
            "https://Shop.Test/a b?x=1&y=%20#frag",
            "https://h/?tokenizer=1&mytoken=2&token",
            "http://h:80/?",
            "https://h/#section-2",
            "https://h/p@q",
        ] {
            assert_eq!(redact(address), address);
        }
    }

    #[test]
    fn redact_handles_percent_encoded_keys_and_repeated_keys() {
        assert_eq!(
            redact("https://h/?%74oken=a&to%6Ben=b&sig=c&sig=d&ok=1"),
            "https://h/?%74oken=REDACTED&to%6Ben=REDACTED&sig=REDACTED&sig=REDACTED&ok=1"
        );
    }

    #[test]
    fn redact_text_finds_urls_inside_error_sentences() {
        let cases = [
            (
                "Page.goto: net::ERR_X at https://h/p?token=abc&x=1",
                "Page.goto: net::ERR_X at https://h/p?token=REDACTED&x=1",
            ),
            (
                "could not open \"https://h/cb?api_key=abc\" in time",
                "could not open \"https://h/cb?api_key=REDACTED\" in time",
            ),
            (
                "failed (see http://h/?sig=abc#access_token=def) twice",
                "failed (see http://h/?sig=REDACTED#access_token=REDACTED) twice",
            ),
            (
                "HTTPS://H/p?Token=abc then http://u:pw@h2/x?password=q and https://h3/?ok=1",
                "HTTPS://H/p?Token=REDACTED then http://h2/x?password=REDACTED and https://h3/?ok=1",
            ),
            ("no address here, token=abc stays", "no address here, token=abc stays"),
            ("ends with a scheme http://", "ends with a scheme http://"),
            (
                "ünïcode before https://h/?token=a\u{e9} after",
                "ünïcode before https://h/?token=REDACTED after",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(redact_text(input), expected, "{input}");
        }
    }

    #[test]
    fn redact_strips_userinfo_if_one_reaches_it() {
        assert_eq!(redact("https://u:p@h/x?token=1"), "https://h/x?token=REDACTED");
        assert_eq!(redact("https://h/a@b"), "https://h/a@b");
    }
}
