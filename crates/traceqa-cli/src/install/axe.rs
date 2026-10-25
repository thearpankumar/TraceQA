//! `traceqa install --axe`: puts axe-core where `traceqa extract --axe` looks for it.
//!
//! axe-core is MPL-2.0, a file-level copyleft licence. TraceQA does not vendor it, embed it in the
//! binary or put it in a release archive. This command downloads the pinned npm tarball from
//! the official registry, at the user's request, checks its SHA-512 against the value pinned in
//! this program, and extracts exactly three files (`axe.min.js`, `LICENSE` and, when present,
//! `LICENSE-3RD-PARTY.txt`) without changing them. It then prints the licence, where the source
//! lives and that the file is unmodified.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use sha2::{Digest, Sha512};
use traceqa_engine::axe::{self, PINNED_VERSION, SOURCE_URL, TARBALL_INTEGRITY_SHA512, TARBALL_URL};
use traceqa_engine::home;

/// Names a different place to download the tarball from (a mirror, or a test server). The pinned
/// SHA-512 is checked all the same, so this cannot make the installer accept other bytes.
const URL_ENV: &str = "TRACEQA_AXE_TARBALL_URL";
/// The tarball is about 700 KB.
const MAX_TARBALL_BYTES: u64 = 16 * 1024 * 1024;
/// A file inside it is at most this big once unpacked. axe.min.js is about 650 KB.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// What is taken from the tarball: its path inside, and whether the install fails without it.
const WANTED: [(&str, &str, bool); 3] = [
    ("package/axe.min.js", "axe.min.js", true),
    ("package/LICENSE", "LICENSE", true),
    ("package/LICENSE-3RD-PARTY.txt", "LICENSE-3RD-PARTY.txt", false),
];

fn download_url() -> String {
    std::env::var(URL_ENV)
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| TARBALL_URL.to_string())
}

pub(super) fn install(dry_run: bool) -> Result<(), String> {
    let home = home::dir().ok_or_else(|| format!("no home folder found. Set {}.", home::HOME_ENV))?;
    let destination = axe::installed_dir(&home);
    let url = download_url();
    if dry_run {
        println!("Dry run: nothing is downloaded or written.");
    }
    let say = |text: &str| {
        if dry_run {
            println!("{text}");
        } else {
            eprintln!("{text}");
        }
    };
    say(&format!(
        "axe-core {PINNED_VERSION} is optional and is not part of TraceQA (licence: MPL-2.0)."
    ));
    say(&format!("  download   {url}"));
    say(&format!("  sha512     {TARBALL_INTEGRITY_SHA512}"));
    say(&format!("  install to {}", destination.display()));
    if dry_run {
        println!("\nDry run finished: nothing was downloaded.");
        return Ok(());
    }
    let bytes = download(&url)?;
    unpack(&bytes, TARBALL_INTEGRITY_SHA512, &destination)?;
    print_notice(&destination);
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!("traceqa/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let agent: ureq::Agent = config.into();
    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("could not download {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_TARBALL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("could not read {url}: {e}"))?;
    if bytes.len() as u64 > MAX_TARBALL_BYTES {
        return Err(format!(
            "{url} is larger than {} MiB; refusing it",
            MAX_TARBALL_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

/// `sha512-<base64>` as npm writes it, without the prefix.
fn sha512_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
}

/// Checks the tarball against `expected` (base64 SHA-512), then writes only the wanted files into
/// `destination`. Nothing is written when the check or the contents fail.
fn unpack(tarball: &[u8], expected: &str, destination: &Path) -> Result<(), String> {
    let actual = sha512_base64(tarball);
    if actual != expected {
        return Err(format!(
            "the download does not match the pinned hash, so it was rejected and nothing was written.\n  expected sha512-{expected}\n  got      sha512-{actual}"
        ));
    }
    let mut found: Vec<Option<Vec<u8>>> = vec![None; WANTED.len()];
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    for entry in archive
        .entries()
        .map_err(|e| format!("the tarball is not readable: {e}"))?
    {
        let entry = entry.map_err(|e| format!("the tarball is not readable: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(|e| format!("the tarball has a bad path: {e}"))?;
        let Some(index) = WANTED
            .iter()
            .position(|(inside, _, _)| path.as_ref() == Path::new(inside))
        else {
            continue;
        };
        let mut content = Vec::new();
        entry
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut content)
            .map_err(|e| format!("the tarball is not readable: {e}"))?;
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("{} in the tarball is too large", WANTED[index].0));
        }
        found[index] = Some(content);
    }
    for ((inside, _, required), content) in WANTED.iter().zip(&found) {
        if *required && content.is_none() {
            return Err(format!("the tarball has no {inside}"));
        }
    }
    fs::create_dir_all(destination).map_err(|e| format!("could not create {}: {e}", destination.display()))?;
    for ((_, name, _), content) in WANTED.iter().zip(found) {
        let target = destination.join(name);
        match content {
            Some(content) => {
                fs::write(&target, content).map_err(|e| format!("could not write {}: {e}", target.display()))?
            }
            // A copy from an earlier install must not outlive the file the new tarball lacks.
            None => {
                let _ = fs::remove_file(&target);
            }
        }
    }
    Ok(())
}

fn print_notice(destination: &Path) {
    let file: PathBuf = destination.join("axe.min.js");
    eprintln!(
        "\nInstalled axe-core {PINNED_VERSION} to {}.\n\n\
         axe-core is licensed under the Mozilla Public License 2.0 (the licence text is next to the file,\n\
         {}). Its source is at {SOURCE_URL}. The file is exactly as published on npm: unmodified,\n\
         its copyright notice intact. TraceQA does not include axe-core in its binary or its release archives.\n\n\
         Use it with: traceqa extract URL --axe\n\
         Automated rules find only part of the accessibility problems a page has; this is not a conformance claim.",
        file.display(),
        destination.join("LICENSE").display(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gzip tarball with the given (path, content) entries, as npm lays one out.
    fn tarball(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, content.as_bytes()).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("traceqa-install-axe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_pinned_hash_is_a_sha512_in_base64() {
        assert_eq!(TARBALL_INTEGRITY_SHA512.len(), 88);
        assert!(TARBALL_URL.starts_with("https://registry.npmjs.org/axe-core/-/axe-core-"));
        assert!(TARBALL_URL.contains(PINNED_VERSION));
    }

    #[test]
    fn a_matching_tarball_writes_only_the_three_files() {
        let bytes = tarball(&[
            ("package/axe.min.js", "AXE"),
            ("package/LICENSE", "MPL"),
            ("package/LICENSE-3RD-PARTY.txt", "THIRD"),
            ("package/package.json", "{}"),
            ("package/locales/de.json", "{}"),
        ]);
        let dest = temp("ok");
        unpack(&bytes, &sha512_base64(&bytes), &dest).unwrap();
        let mut names: Vec<String> = fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["LICENSE", "LICENSE-3RD-PARTY.txt", "axe.min.js"]);
        assert_eq!(fs::read_to_string(dest.join("axe.min.js")).unwrap(), "AXE");
        fs::remove_dir_all(dest).unwrap();
    }

    #[test]
    fn a_tampered_tarball_is_rejected_and_writes_nothing() {
        let bytes = tarball(&[("package/axe.min.js", "AXE"), ("package/LICENSE", "MPL")]);
        let expected = sha512_base64(&bytes);
        let mut tampered = bytes.clone();
        let last = tampered.len() - 3;
        tampered[last] ^= 0xff;
        let dest = temp("tampered");
        let error = unpack(&tampered, &expected, &dest).unwrap_err();
        assert!(error.contains("does not match the pinned hash"), "{error}");
        assert!(!dest.exists(), "nothing is written for rejected bytes");
    }

    #[test]
    fn a_tarball_without_the_script_or_the_licence_is_refused() {
        for entries in [vec![("package/LICENSE", "MPL")], vec![("package/axe.min.js", "AXE")]] {
            let bytes = tarball(&entries);
            let dest = temp("incomplete");
            let error = unpack(&bytes, &sha512_base64(&bytes), &dest).unwrap_err();
            assert!(error.contains("has no package/"), "{error}");
            assert!(!dest.exists());
        }
    }
}
