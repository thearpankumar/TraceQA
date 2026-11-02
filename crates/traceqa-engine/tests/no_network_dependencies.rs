//! The engine core must not depend on anything that can reach a network or a model (backlog EXP-6): a model sits
//! behind a trait, and what implements it lives in `traceqa-models`. This runs `cargo tree -p traceqa-engine`, the
//! command the backlog names, and fails when one of those crates shows up, so that a stray dependency is caught by
//! the build and not by a review.

use std::process::Command;

/// Crates that make network requests or talk to a model, and the crate that holds the sources. `ureq` is the one the
/// rest of the workspace uses for `traceqa map`.
const FORBIDDEN: [&str; 14] = [
    "ureq",
    "reqwest",
    "hyper",
    "h2",
    "isahc",
    "curl",
    "attohttpc",
    "rustls",
    "native-tls",
    "openssl",
    "tokio",
    "async-std",
    "aws-sdk-bedrockruntime",
    "traceqa-models",
];

#[test]
fn the_engine_depends_on_no_http_client_and_no_model_crate() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(["tree", "-p", "traceqa-engine", "--prefix", "none", "--locked"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree could not be started");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(tree.contains("traceqa-engine"), "the tree looks empty: {tree}");
    let found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| FORBIDDEN.contains(name))
        .collect();
    assert!(found.is_empty(), "the engine depends on {found:?}:\n{tree}");
}
