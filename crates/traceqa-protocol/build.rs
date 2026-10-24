//! Generates Rust types from the JSON Schemas in `spec/`.
//!
//! Every `spec/*.schema.json` file contributes its `$defs` to one shared
//! namespace, so type names must be unique across files. The `x-traceqa`
//! block of each file supplies the contract version and method names.

// A build script that cannot read or generate its input must stop the build, and a panic is how
// it does that. Nothing here runs in a shipped program.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use serde_json::{json, Map, Value};

fn main() {
    let spec_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../spec");
    println!("cargo:rerun-if-changed={}", spec_dir.display());

    let mut files: Vec<PathBuf> = fs::read_dir(&spec_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", spec_dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".schema.json"))
        .collect();
    files.sort();

    let mut definitions = Map::new();
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut meta = String::new();

    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let schema = read_json(path);

        for (name, def) in schema["$defs"].as_object().into_iter().flatten() {
            if let Some(first) = owner.insert(name.clone(), file_name.clone()) {
                panic!("type {name} is defined in both {first} and {file_name}");
            }
            let mut def = def.clone();
            flatten_refs(&mut def);
            definitions.insert(name.clone(), def);
        }

        write_contract_meta(&mut meta, &file_name, &schema["x-traceqa"]);
    }

    let root = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "definitions": definitions,
    });
    let root: schemars::schema::RootSchema = serde_json::from_value(root).unwrap();

    let mut settings = typify::TypeSpaceSettings::default();
    settings.with_derive("PartialEq".to_string());
    let mut type_space = typify::TypeSpace::new(&settings);
    type_space
        .add_root_schema(root)
        .expect("spec/ does not convert to Rust types");
    let types = prettyplease::unparse(&syn::parse2::<syn::File>(type_space.to_stream()).unwrap());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("spec_types.rs"), types).unwrap();
    fs::write(out_dir.join("spec_meta.rs"), meta).unwrap();
    fs::write(out_dir.join("spec_parse_as.rs"), parse_as_source(owner.keys())).unwrap();
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Rewrites `other.schema.json#/$defs/Name` and `#/$defs/Name` to `#/definitions/Name`.
fn flatten_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                let name = reference.rsplit('/').next().unwrap().to_string();
                *reference = format!("#/definitions/{name}");
            }
            map.values_mut().for_each(flatten_refs);
        }
        Value::Array(items) => items.iter_mut().for_each(flatten_refs),
        _ => {}
    }
}

fn write_contract_meta(out: &mut String, file_name: &str, x: &Value) {
    let contract = x["contract"]
        .as_str()
        .unwrap_or_else(|| panic!("{file_name}: missing x-traceqa.contract"));
    let version = x["version"]
        .as_u64()
        .unwrap_or_else(|| panic!("{file_name}: missing x-traceqa.version"));

    writeln!(out, "/// Contract `{contract}`, from `spec/{file_name}`.").unwrap();
    writeln!(out, "pub mod {} {{", contract.replace('-', "_")).unwrap();
    writeln!(out, "    pub const VERSION: u64 = {version};").unwrap();
    for (key, module) in [("methods", "method"), ("notifications", "notification")] {
        let Some(names) = x[key].as_object() else { continue };
        writeln!(out, "    pub mod {module} {{").unwrap();
        for name in names.keys() {
            let ident = name.replace(['.', '-'], "_").to_uppercase();
            writeln!(out, "        pub const {ident}: &str = {name:?};").unwrap();
        }
        writeln!(out, "    }}").unwrap();
    }
    writeln!(out, "}}").unwrap();
}

/// Source of `parse_as`, which the example test includes to check a JSON document
/// against a type chosen by name. It is kept out of the library.
fn parse_as_source<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let mut out = String::from(
        "/// Parses `json` as the spec type called `type_name` and serialises it back.\n\
         /// Returns `None` when there is no such type.\n\
         fn parse_as(type_name: &str, json: &str) -> Option<serde_json::Result<serde_json::Value>> {\n\
         \x20   Some(match type_name {\n",
    );
    for name in names {
        writeln!(
            out,
            "        {name:?} => serde_json::from_str::<traceqa_protocol::types::{name}>(json).and_then(serde_json::to_value),"
        )
        .unwrap();
    }
    out.push_str("        _ => return None,\n    })\n}\n");
    out
}
