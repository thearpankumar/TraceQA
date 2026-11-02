//! What the explorer may touch.
//!
//! The driver keeps a browser from doing damage whatever a page says (`ActPolicy`, enforced on the browser's own
//! requests). This module is the layer above it: it decides which components the explorer should not even try, so
//! that a click is not wasted on, and a report is not full of, things the driver would block anyway. It is plain
//! functions over plain data: no browser, no file, no clock.
//!
//! It errs toward skipping. A skipped component counts as found and not touched, so a wrong skip costs coverage and
//! a wrong click could cost a customer's data. The deny-list matches whole words by their beginning, so `delete`
//! also catches `deleted` and `payment` is caught by `pay`; it also catches `Payment history`, which is harmless to
//! skip. Words in other languages that are inflected differently from the listed forms are missed. That is why the
//! driver's refusal to send anything but GET and HEAD, and not this list, is what the safety rests on.

use traceqa_common::types::ActPolicy;
use unicode_normalization::UnicodeNormalization;
use url::{Host, Url};

/// Words that mean an action with a lasting effect, by language. Each entry is one word, or a phrase of words that
/// must follow one another. Matching is by the beginning of each word (see the module note), in lower case and after
/// Unicode normalisation, with Arabic diacritics left out. These lists were written for TraceQA and have not been
/// reviewed by native speakers of every language: review a language before relying on it.
const DENY_NAMES: &[(&str, &[&str])] = &[
    (
        "en",
        &[
            "delete",
            "remove",
            "pay",
            "buy",
            "purchase",
            "order",
            "submit",
            "send",
            "sign out",
            "log out",
            "logout",
            "signout",
            "cancel subscription",
            "unsubscribe",
            "checkout",
            "clear",
            "empty",
            "donate",
            "transfer",
            "withdraw",
        ],
    ),
    (
        "es",
        &[
            "eliminar",
            "borrar",
            "quitar",
            "pagar",
            "pago",
            "comprar",
            "pedido",
            "enviar",
            "cerrar sesión",
            "cerrar sesion",
            "cancelar suscripción",
            "cancelar suscripcion",
            "darse de baja",
            "vaciar",
            "finalizar compra",
            "donar",
        ],
    ),
    (
        "fr",
        &[
            "supprimer",
            "effacer",
            "retirer",
            "payer",
            "paiement",
            "acheter",
            "commander",
            "envoyer",
            "déconnexion",
            "se déconnecter",
            "annuler l'abonnement",
            "se désabonner",
            "vider",
            "faire un don",
        ],
    ),
    (
        "de",
        &[
            "löschen",
            "entfernen",
            "bezahlen",
            "zahlung",
            "kaufen",
            "bestellen",
            "senden",
            "absenden",
            "abmelden",
            "ausloggen",
            "abo kündigen",
            "abbestellen",
            "leeren",
            "zur kasse",
            "spenden",
        ],
    ),
    (
        "hi",
        &[
            "हटाएं",
            "हटाएँ",
            "हटाओ",
            "मिटाएं",
            "मिटाएँ",
            "डिलीट",
            "भुगतान",
            "पेमेंट",
            "खरीद",
            "ऑर्डर",
            "भेजें",
            "साइन आउट",
            "लॉग आउट",
            "सदस्यता रद्द",
            "अनसब्सक्राइब",
            "साफ़ करें",
        ],
    ),
    (
        "ar",
        &[
            "حذف",
            "إزالة",
            "ازالة",
            "أزل",
            "ازل",
            "دفع",
            "ادفع",
            "شراء",
            "اشتر",
            "اشتري",
            "طلب",
            "إرسال",
            "ارسال",
            "أرسل",
            "ارسل",
            "تسجيل الخروج",
            "تسجيل خروج",
            "إلغاء الاشتراك",
            "الغاء الاشتراك",
            "إفراغ",
            "افراغ",
        ],
    ),
];

/// Parts of an address that mean an action with a lasting effect. A link such as `/logout` or `?action=delete` is a
/// plain GET, which the driver lets through, so the address is judged here.
const DENY_ADDRESS_WORDS: &[&str] = &[
    "logout",
    "signout",
    "delete",
    "remove",
    "unsubscribe",
    "checkout",
    "payment",
    "pay",
    "purchase",
];

/// Why a component is left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Its name says it does something with a lasting effect.
    Destructive(String),
    /// Its address says so.
    DestructiveLink(String),
    /// It leads away from the origin the exploration was given, or somewhere that is not a page.
    OtherOrigin,
    /// Read-only mode only touches links, tabs and things that open and close.
    NotReadOnlySafe,
}

impl Skip {
    /// The reason as `coverage.json` words it (backlog EXP-7): `destructive`, `blocked` or `read-only`.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Destructive(_) | Self::DestructiveLink(_) => "destructive",
            Self::OtherOrigin => "blocked",
            Self::NotReadOnlySafe => "read-only",
        }
    }
}

/// What to do with a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Skip(Skip),
}

/// The facts about one component that the rules look at.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub role: &'a str,
    pub name: &'a str,
    /// The `href` attribute, as the page wrote it.
    pub href: Option<&'a str>,
    /// `Some` when the component opens and closes something (`aria-expanded`).
    pub expanded: Option<bool>,
    /// The address of the page the component is on, which a relative `href` is read against.
    pub page_url: &'a str,
}

/// The rules of one exploration.
#[derive(Debug, Clone)]
pub struct Rules {
    /// The origin the exploration starts at, and then the others it may go to, without repeats.
    origins: Vec<String>,
    allow_writes: bool,
    /// Words added by the person (`--deny-name`), already normalised.
    extra: Vec<Vec<String>>,
}

impl Rules {
    /// Rules for an exploration that starts at `start_url`, which must be an http or https address. `extra_deny`
    /// are words or phrases to add to the lists above.
    pub fn new(start_url: &str, allow_writes: bool, extra_deny: &[String]) -> Result<Self, String> {
        Ok(Self {
            origins: vec![origin_of(start_url).ok_or_else(|| format!("{start_url:?} is not an http or https address"))?],
            allow_writes,
            extra: extra_deny
                .iter()
                .map(|term| words(term))
                .filter(|w| !w.is_empty())
                .collect(),
        })
    }

    /// As [`new`](Self::new), and the exploration may also go to `allow_origins` (each an http or https address; only
    /// its origin counts). Allowing writes is a statement about where, so it needs the start origin to be named
    /// among `allow_origins`: a person who allows writes has to say which site may be written to.
    pub fn with_origins(
        start_url: &str,
        allow_origins: &[String],
        allow_writes: bool,
        extra_deny: &[String],
    ) -> Result<Self, String> {
        let mut rules = Self::new(start_url, allow_writes, extra_deny)?;
        let mut named = Vec::new();
        for address in allow_origins {
            let origin = origin_of(address).ok_or_else(|| format!("{address:?} is not an http or https address"))?;
            if !named.contains(&origin) {
                named.push(origin);
            }
        }
        if allow_writes && !named.contains(&rules.origins[0]) {
            return Err(format!(
                "--allow-writes needs --allow-origin {} to say that this site may be written to",
                rules.origins[0]
            ));
        }
        for origin in named {
            if !rules.origins.contains(&origin) {
                rules.origins.push(origin);
            }
        }
        Ok(rules)
    }

    /// The policy to give the driver when it opens the session of this exploration.
    pub fn driver_policy(&self) -> ActPolicy {
        ActPolicy {
            allow_origins: self.origins.clone(),
            allow_writes: self.allow_writes.then_some(true),
        }
    }

    /// Whether the explorer should act on this component. A name or an address that says "destructive" always
    /// wins, even with writes allowed: that is what the explorer is for, finding things, not buying them.
    pub fn decide(&self, candidate: &Candidate) -> Decision {
        if let Some(term) = self.deny_name(candidate.name) {
            return Decision::Skip(Skip::Destructive(term));
        }
        if let Some(href) = candidate.href {
            let Some(target) = resolve(candidate.page_url, href) else {
                return Decision::Skip(Skip::OtherOrigin);
            };
            if let Some(word) = deny_address(&target) {
                return Decision::Skip(Skip::DestructiveLink(word));
            }
            if !origin_of(target.as_str()).is_some_and(|origin| self.origins.contains(&origin)) {
                return Decision::Skip(Skip::OtherOrigin);
            }
        }
        if !self.allow_writes && !read_only_safe(candidate) {
            return Decision::Skip(Skip::NotReadOnlySafe);
        }
        Decision::Allow
    }

    /// The listed word or phrase that `name` contains, if any.
    pub fn deny_name(&self, name: &str) -> Option<String> {
        let found = words(name);
        let listed = DENY_NAMES
            .iter()
            .flat_map(|(_, terms)| terms.iter())
            .map(|term| words(term));
        listed
            .chain(self.extra.iter().cloned())
            .find(|term| contains_phrase(&found, term))
            .map(|term| term.join(" "))
    }
}

/// Read-only mode touches only: links (their address was already checked), tabs, and anything that opens and closes.
fn read_only_safe(candidate: &Candidate) -> bool {
    (candidate.role == "link" && candidate.href.is_some()) || candidate.role == "tab" || candidate.expanded.is_some()
}

/// Whether the person's explorer may start here without being told it is allowed to interact: loopback and the names
/// reserved for tests and local use. Anything else needs an explicit statement that the person understands that the
/// explorer interacts with the site.
pub fn is_local_origin(address: &str) -> bool {
    let Ok(url) = Url::parse(address) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.octets()[0] == 127,
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        // `Url` lower-cases a domain, so `LOCALHOST` is already `localhost`.
        Some(Host::Domain(name)) => name == "localhost" || name.ends_with(".localhost") || name.ends_with(".test"),
        None => false,
    }
}

/// Whether an exploration may start at `start_url` and go to `allow_origins`: every one of them must be local, unless
/// the person has said they understand that the explorer interacts with a site. The message names the flag.
pub fn check_start(start_url: &str, allow_origins: &[String], i_understand: bool) -> Result<(), String> {
    if origin_of(start_url).is_none() {
        return Err(format!("{start_url:?} is not an http or https address"));
    }
    if i_understand {
        return Ok(());
    }
    for address in std::iter::once(start_url).chain(allow_origins.iter().map(String::as_str)) {
        if !is_local_origin(address) {
            return Err(format!(
                "{address} is not on this machine. The explorer clicks and types on the site it explores, so it only \
                 starts on localhost, 127.0.0.0/8, ::1, *.localhost and *.test unless you pass \
                 --i-understand-this-interacts"
            ));
        }
    }
    Ok(())
}

/// The policy the driver was asked to keep is the one it says it keeps. A driver that does not know about policies
/// sends none back, and a session that is not known to be protected must not be acted on.
pub fn enforced(asked: &ActPolicy, got: Option<&ActPolicy>) -> bool {
    let Some(got) = got else {
        return false;
    };
    let mut asked_origins = asked.allow_origins.clone();
    let mut got_origins = got.allow_origins.clone();
    asked_origins.sort();
    got_origins.sort();
    asked_origins == got_origins && asked.allow_writes.unwrap_or(false) == got.allow_writes.unwrap_or(false)
}

/// `scheme://host[:port]` of an http or https address, the way a browser writes an origin. `None` for anything else.
fn origin_of(address: &str) -> Option<String> {
    let url = Url::parse(address).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

fn resolve(page_url: &str, href: &str) -> Option<Url> {
    Url::parse(page_url).ok()?.join(href.trim()).ok()
}

/// Splits text into words: lower case, composed, without Arabic diacritics, cut at white space and punctuation.
/// Punctuation that is not ASCII is listed by hand, because the standard library has no test for it.
fn words(text: &str) -> Vec<String> {
    const PUNCTUATION: &str = "،؛؟।॥“”‘’«»…–—·";
    let cleaned: String = text
        .nfc()
        .flat_map(char::to_lowercase)
        // Arabic vowel marks and the stretching letter: a name written with them is the same name.
        .filter(|c| !matches!(*c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}'))
        .collect();
    cleaned
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation() || PUNCTUATION.contains(c))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `found` holds the words of `term` in order, each beginning like the word of the term, with at most one
/// other word among them: `cancel your subscription` is `cancel subscription`.
fn contains_phrase(found: &[String], term: &[String]) -> bool {
    let begins = |word: &String, wanted: &String| word.starts_with(wanted.as_str());
    if term.is_empty() {
        return false;
    }
    let together = found
        .windows(term.len())
        .any(|window| window.iter().zip(term).all(|(word, wanted)| begins(word, wanted)));
    // One word in the middle that the term does not have. A single word term has no middle, so `1..1` is empty.
    let apart = found.windows(term.len() + 1).any(|window| {
        (1..term.len()).any(|skipped| {
            window
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != skipped)
                .map(|(_, word)| word)
                .zip(term)
                .all(|(word, wanted)| begins(word, wanted))
        })
    });
    together || apart
}

/// The word of `DENY_ADDRESS_WORDS` that the path or the query of an address holds, if any. Words are cut at `/`, `-`,
/// `_`, `.`, `=`, `&` and `+`, and two neighbours may make one (`log-out`, `sign_out`).
fn deny_address(url: &Url) -> Option<String> {
    let mut text = url.path().to_string();
    if let Some(query) = url.query() {
        text.push('/');
        text.push_str(query);
    }
    let lower = text.to_lowercase();
    let parts: Vec<&str> = lower
        .split(['/', '-', '_', '.', '=', '&', '+', '?', '%'])
        .filter(|part| !part.is_empty())
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let joined = parts.get(index + 1).map(|next| format!("{part}{next}"));
        for listed in DENY_ADDRESS_WORDS {
            if part == listed || joined.as_deref() == Some(*listed) {
                return Some((*listed).to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "http://127.0.0.1:4173/";

    fn rules() -> Rules {
        Rules::new(SHOP, false, &[]).unwrap()
    }

    fn link<'a>(name: &'a str, href: &'a str) -> Candidate<'a> {
        Candidate {
            role: "link",
            name,
            href: Some(href),
            expanded: None,
            page_url: SHOP,
        }
    }

    fn button(name: &str) -> Candidate<'_> {
        Candidate {
            role: "button",
            name,
            href: None,
            expanded: None,
            page_url: SHOP,
        }
    }

    fn denied(name: &str) -> bool {
        rules().deny_name(name).is_some()
    }

    #[test]
    fn english_names_that_do_something_lasting_are_listed() {
        for name in [
            "Delete account",
            "Remove",
            "Pay now",
            "Buy now",
            "Place order",
            "Purchase",
            "Submit",
            "Send message",
            "Sign out",
            "Log out",
            "Logout",
            "Cancel subscription",
            "Unsubscribe",
            "Checkout",
            "Clear cart",
            "Empty the cart",
            "Donate",
            "Transfer funds",
            "Withdraw",
            "DELETE",
            "  delete   all  ",
        ] {
            assert!(denied(name), "{name:?} should be denied");
        }
    }

    #[test]
    fn english_names_that_only_look_are_not() {
        for name in [
            "Add to cart",
            "Next page",
            "Products",
            "Search",
            "Home",
            "Contact us",
            "Open size guide",
            "Display settings",
            "Read more",
            "View details",
            "Sort by price",
            "",
        ] {
            assert!(!denied(name), "{name:?} should not be denied");
        }
    }

    #[test]
    fn a_word_is_matched_by_its_beginning_and_not_inside_another_word() {
        assert_eq!(rules().deny_name("Deleted items").as_deref(), Some("delete"));
        // "pay" is the start of "payment" ...
        assert!(denied("Payment history"));
        // ... but not found inside "display", "spay" or "prepay".
        assert!(!denied("Display"));
        assert!(!denied("Prepay"));
    }

    #[test]
    fn punctuation_that_is_not_ascii_separates_words_too() {
        // Arabic comma, em dash, ellipsis and the Devanagari danda, with no space around them.
        for name in [
            "Hello\u{060C}delete",
            "Cart\u{2014}delete",
            "Wait\u{2026}delete",
            "ठीक\u{0964}delete",
            "Go\u{00AB}delete",
        ] {
            assert!(denied(name), "{name:?} should be denied");
        }
        assert!(!denied("Hello\u{060C}world"));
    }

    #[test]
    fn a_phrase_needs_its_words_in_order_with_at_most_one_word_between() {
        assert!(denied("Please sign out here"));
        assert!(denied("Cancel your subscription"));
        assert!(denied("Cancel subscription"));
        // Two words between, or the wrong order, is not the phrase.
        assert!(!denied("Sign in or out of the game"));
        assert!(!denied("Out, sign"));
        assert!(!denied("Subscription cancel"));
    }

    #[test]
    fn spanish_french_german_hindi_and_arabic_names_are_listed() {
        for name in [
            // Spanish
            "Eliminar cuenta",
            "Pagar ahora",
            "Comprar",
            "Cerrar sesión",
            "Vaciar carrito",
            // French
            "Supprimer le compte",
            "Payer maintenant",
            "Acheter",
            "Se déconnecter",
            "Annuler l'abonnement",
            // German
            "Konto löschen",
            "Jetzt bezahlen",
            "Kaufen",
            "Abmelden",
            "Zur Kasse",
            // Hindi
            "खाता हटाएं",
            "अभी भुगतान करें",
            "खरीदें",
            "साइन आउट",
            "कार्ट साफ़ करें",
            // Arabic
            "حذف الحساب",
            "ادفع الآن",
            "شراء",
            "تسجيل الخروج",
            "إلغاء الاشتراك",
        ] {
            assert!(denied(name), "{name:?} should be denied");
        }
    }

    #[test]
    fn names_in_those_languages_that_only_look_are_not_listed() {
        for name in [
            "Inicio",
            "Productos",
            "Contacto",
            "Accueil",
            "Produits",
            "Startseite",
            "Kontakt",
            "होम",
            "उत्पाद",
            "संपर्क करें",
            "الرئيسية",
            "المنتجات",
            "اتصل بنا",
        ] {
            assert!(!denied(name), "{name:?} should not be denied");
        }
    }

    #[test]
    fn arabic_with_vowel_marks_and_hindi_in_either_composition_still_match() {
        assert!(denied("اِدْفَعْ"));
        // "फ़" as one character (U+095E) and as two (U+092B and the nukta U+093C), after "सा": "साफ़".
        assert!(denied("सा\u{095E} करें"));
        assert!(denied("सा\u{092B}\u{093C} करें"));
    }

    #[test]
    fn the_person_can_add_words() {
        let rules = Rules::new(SHOP, false, &["Archive".to_string(), "  ".to_string()]).unwrap();
        assert_eq!(rules.deny_name("Archive this").as_deref(), Some("archive"));
        assert!(rules.deny_name("Home").is_none());
    }

    #[test]
    fn an_address_that_does_something_lasting_is_listed() {
        for href in [
            "/logout",
            "/account/delete",
            "/Sign-Out",
            "/log-out",
            "/sign_out",
            "/shop?action=delete",
            "/shop?do=remove&id=3",
            "/checkout/step1",
            "/cart/payment",
            "/unsubscribe?u=1",
        ] {
            let decision = rules().decide(&link("Account", href));
            assert!(
                matches!(decision, Decision::Skip(Skip::DestructiveLink(_))),
                "{href}: {decision:?}"
            );
        }
    }

    #[test]
    fn an_address_that_only_looks_is_not() {
        for href in [
            "/products/blue-shirt",
            "/blog/payroll-news",
            "/delivery",
            "/about",
            "/shop?sort=price",
            "/removal-guide",
        ] {
            let decision = rules().decide(&link("Read", href));
            assert_eq!(decision, Decision::Allow, "{href}");
        }
    }

    #[test]
    fn a_link_to_another_origin_or_not_to_a_page_is_blocked() {
        for href in [
            "https://example.com/",
            "http://127.0.0.1:9999/",
            "https://127.0.0.1:4173/",
            "mailto:me@example.com",
            "javascript:void(0)",
            "tel:+911234",
        ] {
            let decision = rules().decide(&link("Elsewhere", href));
            assert_eq!(decision, Decision::Skip(Skip::OtherOrigin), "{href}");
        }
    }

    #[test]
    fn a_relative_link_is_read_against_the_page_it_is_on() {
        let on = |page: &'static str, href| Candidate {
            page_url: page,
            ..link("Next", href)
        };
        assert_eq!(
            rules().decide(&on("http://127.0.0.1:4173/shop/", "../about")),
            Decision::Allow
        );
        assert_eq!(
            rules().decide(&on("http://127.0.0.1:4173/shop/", "//evil.test/x")),
            Decision::Skip(Skip::OtherOrigin)
        );
        // A page address nobody can read: the link is not trusted.
        assert_eq!(
            rules().decide(&on("not an address", "/about")),
            Decision::Skip(Skip::OtherOrigin)
        );
    }

    #[test]
    fn read_only_touches_only_links_tabs_and_things_that_open_and_close() {
        let r = rules();
        assert_eq!(r.decide(&link("Next", "/next")), Decision::Allow);
        let tab = Candidate {
            role: "tab",
            ..button("Details")
        };
        assert_eq!(r.decide(&tab), Decision::Allow);
        let disclosure = Candidate {
            expanded: Some(false),
            ..button("More")
        };
        assert_eq!(r.decide(&disclosure), Decision::Allow);
        let opened = Candidate {
            expanded: Some(true),
            ..button("Less")
        };
        assert_eq!(r.decide(&opened), Decision::Allow);
        assert_eq!(r.decide(&button("Add to cart")), Decision::Skip(Skip::NotReadOnlySafe));
        let field = Candidate {
            role: "textbox",
            ..button("Search")
        };
        assert_eq!(r.decide(&field), Decision::Skip(Skip::NotReadOnlySafe));
        // A link with no address is not a link the rules can judge.
        let bare = Candidate {
            href: None,
            ..link("Go", "/x")
        };
        assert_eq!(r.decide(&bare), Decision::Skip(Skip::NotReadOnlySafe));
    }

    #[test]
    fn allowing_writes_opens_the_buttons_and_never_the_deny_list() {
        let r = Rules::new(SHOP, true, &[]).unwrap();
        assert_eq!(r.decide(&button("Add to cart")), Decision::Allow);
        assert!(matches!(
            r.decide(&button("Delete account")),
            Decision::Skip(Skip::Destructive(_))
        ));
        assert!(matches!(
            r.decide(&link("Account", "/logout")),
            Decision::Skip(Skip::DestructiveLink(_))
        ));
        // Other origins stay out of reach.
        assert_eq!(
            r.decide(&link("Elsewhere", "https://example.com/")),
            Decision::Skip(Skip::OtherOrigin)
        );
    }

    #[test]
    fn the_deny_list_wins_over_everything_that_would_allow() {
        let r = rules();
        assert!(matches!(
            r.decide(&link("Clear cart", "/cart")),
            Decision::Skip(Skip::Destructive(_))
        ));
        let toggle = Candidate {
            expanded: Some(false),
            ..button("Delete everything")
        };
        assert!(matches!(r.decide(&toggle), Decision::Skip(Skip::Destructive(_))));
    }

    #[test]
    fn the_reasons_are_the_words_coverage_uses() {
        assert_eq!(Skip::Destructive("x".into()).reason(), "destructive");
        assert_eq!(Skip::DestructiveLink("x".into()).reason(), "destructive");
        assert_eq!(Skip::OtherOrigin.reason(), "blocked");
        assert_eq!(Skip::NotReadOnlySafe.reason(), "read-only");
    }

    #[test]
    fn local_origins_are_loopback_and_the_names_kept_for_tests() {
        for address in [
            "http://localhost:3000",
            "https://LOCALHOST/",
            "http://127.0.0.1:4173",
            "http://127.5.5.5/",
            "http://[::1]:8080/",
            "http://shop.test",
            "http://app.localhost:8000/x",
        ] {
            assert!(is_local_origin(address), "{address}");
        }
    }

    #[test]
    fn everything_else_is_not_local() {
        for address in [
            "https://example.com",
            "http://10.0.0.5",
            "http://192.168.1.2",
            "http://169.254.169.254",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://notlocalhost",
            "http://shop.test.evil.com",
            "http://[::2]/",
            "ftp://localhost",
            "file:///etc/passwd",
            "not an address",
            "",
        ] {
            assert!(!is_local_origin(address), "{address}");
        }
    }

    #[test]
    fn rules_start_only_from_an_http_address_and_give_the_driver_its_policy() {
        assert!(Rules::new("ftp://x.test/", false, &[]).is_err());
        assert!(Rules::new("nonsense", false, &[]).is_err());
        let policy = Rules::new("http://Shop.test:8080/path?q=1", false, &[])
            .unwrap()
            .driver_policy();
        assert_eq!(policy.allow_origins, ["http://shop.test:8080"]);
        assert_eq!(policy.allow_writes, None);
        let writes = Rules::new("https://127.0.0.1/", true, &[]).unwrap().driver_policy();
        assert_eq!(writes.allow_origins, ["https://127.0.0.1"]);
        assert_eq!(writes.allow_writes, Some(true));
    }

    #[test]
    fn more_origins_can_be_allowed_and_the_driver_is_told_about_all_of_them() {
        let rules = Rules::with_origins(
            "http://127.0.0.1:4173/",
            &[
                "http://127.0.0.1:4174".to_string(),
                "http://127.0.0.1:4173/anything".to_string(),
            ],
            false,
            &[],
        )
        .unwrap();
        let mut origins = rules.driver_policy().allow_origins;
        origins.sort();
        assert_eq!(origins, ["http://127.0.0.1:4173", "http://127.0.0.1:4174"]);
        // A link to the second origin is allowed; one to an origin nobody named is not.
        let page = "http://127.0.0.1:4173/";
        let to = |href: &'static str| Candidate {
            role: "link",
            name: "Go",
            href: Some(href),
            expanded: None,
            page_url: page,
        };
        assert_eq!(rules.decide(&to("http://127.0.0.1:4174/frame")), Decision::Allow);
        assert_eq!(
            rules.decide(&to("http://127.0.0.1:4175/")),
            Decision::Skip(Skip::OtherOrigin)
        );
    }

    #[test]
    fn writes_need_the_origin_that_may_be_written_to_to_be_named() {
        let start = "http://127.0.0.1:4173/";
        let none: [String; 0] = [];
        assert!(Rules::with_origins(start, &none, true, &[])
            .unwrap_err()
            .contains("--allow-origin"));
        // Naming only some other origin is not naming the start.
        assert!(Rules::with_origins(start, &["http://127.0.0.1:4174".to_string()], true, &[]).is_err());
        let ok = Rules::with_origins(start, &["http://127.0.0.1:4173".to_string()], true, &[]).unwrap();
        assert_eq!(ok.driver_policy().allow_writes, Some(true));
        // Without writes no origin has to be named.
        assert_eq!(
            Rules::with_origins(start, &none, false, &[])
                .unwrap()
                .driver_policy()
                .allow_writes,
            None
        );
    }

    #[test]
    fn an_origin_that_is_not_an_http_address_is_refused() {
        for bad in ["ftp://x.test", "nonsense", "", "file:///etc"] {
            assert!(
                Rules::with_origins("http://127.0.0.1:4173/", &[bad.to_string()], false, &[]).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_site_that_is_not_local_is_refused_unless_the_person_says_they_understand() {
        let none: [String; 0] = [];
        let refused = check_start("https://example.com/", &none, false).unwrap_err();
        assert!(refused.contains("--i-understand-this-interacts"), "{refused}");
        assert!(check_start("https://example.com/", &none, true).is_ok());
        assert!(check_start("http://127.0.0.1:4173/", &none, false).is_ok());
        assert!(check_start("http://shop.test/", &none, false).is_ok());
        // Every origin the walk may go to counts, not only the first.
        let away = ["https://example.com".to_string()];
        assert!(check_start("http://127.0.0.1:4173/", &away, false).is_err());
        assert!(check_start("http://127.0.0.1:4173/", &away, true).is_ok());
        assert!(check_start("not an address", &none, true).is_err());
    }

    #[test]
    fn a_session_counts_as_protected_only_when_the_driver_says_back_the_same_policy() {
        let asked = ActPolicy {
            allow_origins: vec!["http://a.test".into(), "http://b.test".into()],
            allow_writes: None,
        };
        assert!(enforced(&asked, Some(&asked)));
        // The order of the origins does not matter, and an absent flag is false.
        let shuffled = ActPolicy {
            allow_origins: vec!["http://b.test".into(), "http://a.test".into()],
            allow_writes: Some(false),
        };
        assert!(enforced(&asked, Some(&shuffled)));
        // A driver that does not know about policies says nothing, and that is not protection.
        assert!(!enforced(&asked, None));
        let writes = ActPolicy {
            allow_writes: Some(true),
            ..asked.clone()
        };
        assert!(!enforced(&asked, Some(&writes)));
        let fewer = ActPolicy {
            allow_origins: vec!["http://a.test".into()],
            ..asked.clone()
        };
        assert!(!enforced(&asked, Some(&fewer)));
        let more = ActPolicy {
            allow_origins: vec!["http://a.test".into(), "http://b.test".into(), "http://c.test".into()],
            ..asked.clone()
        };
        assert!(!enforced(&asked, Some(&more)));
    }
}
