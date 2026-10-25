//! Response formats, describe_page, find, output schemas and resources.

use super::*;

/// Says hello as a host that speaks `version`.
fn hello(host: &mut Host, version: &str) {
    let answer = host.request("initialize", json!({ "protocolVersion": version, "capabilities": {} }));
    assert_eq!(answer["result"]["protocolVersion"], version);
    host.write(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
}

fn instance(key: &str, name: &str, attrs: Value, state: Value) -> Value {
    json!({
        "key": key, "name": name, "tag": "button", "attrs": attrs, "selectors": [format!("#{key}")],
        "box": { "x": 0, "y": 0, "w": 10, "h": 10 }, "state": state, "frameId": "main", "inShadowDom": false
    })
}

fn shown() -> Value {
    json!({ "visible": true, "enabled": true, "inViewport": true })
}

/// A component with one instance per name in `names`.
fn component(id: &str, role: &str, name: &str, path: &str, names: &[&str]) -> Value {
    let instances: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(n, name)| instance(&format!("{id}:{n}"), name, json!({}), shown()))
        .collect();
    json!({
        "id": id, "role": role, "name": name, "nameIsTemplate": name == "*", "confidence": "high",
        "signals": ["native"], "path": path, "instances": instances
    })
}

/// Writes `components.json` for `dir`, copied from the example run's, and lists the target.
fn set_components(runs: &Path, dir: &str, components: Vec<Value>) {
    let from = runs.join(RUN_ID).join("firefox-1280x800").join("components.json");
    let mut file: Value = serde_json::from_str(&fs::read_to_string(from).unwrap()).unwrap();
    file["components"] = Value::Array(components);
    let folder = runs.join(RUN_ID).join(dir);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("components.json"), file.to_string()).unwrap();
    let manifest_path = runs.join(RUN_ID).join("run.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let targets = manifest["targets"].as_array_mut().unwrap();
    if !targets.iter().any(|t| t["dir"] == dir) {
        let mut added = targets[0].clone();
        added["dir"] = json!(dir);
        targets.push(added);
    }
    fs::write(manifest_path, manifest.to_string()).unwrap();
}

fn outline_components() -> Vec<Value> {
    vec![
        component(
            "c_000000000001",
            "link",
            "Home",
            "banner > navigation \"Main\"",
            &["Home"],
        ),
        component("c_000000000002", "button", "Cart", "banner", &["Cart"]),
        component(
            "c_000000000003",
            "button",
            "*",
            "main > list \"products\" > listitem",
            &["Item 1", "Item 2", "Item 3", "Item 4", "Item 5"],
        ),
        component(
            "c_000000000004",
            "textbox",
            "Search",
            "main > form \"Find\"",
            &["Search"],
        ),
        component("c_000000000005", "link", "Terms", "contentinfo", &["Terms"]),
        component("c_000000000006", "button", "Loose", "list \"x\" > listitem", &["Loose"]),
        component("c_000000000007", "button", "Bare", "", &["Bare"]),
        component(
            "c_000000000008",
            "button",
            "Odd",
            "region \"a > b \\\" c\" > button",
            &["Odd"],
        ),
        component(
            "c_000000000009",
            "button",
            "Pay",
            "frame \"pay\" > main > form",
            &["Pay"],
        ),
    ]
}

fn groups_of(answer: &Value) -> Vec<String> {
    text_of(answer)["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["group"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn describe_page_groups_components_by_their_first_landmark() {
    let runs = runs_folder("describe");
    set_components(&runs, "firefox-1280x800", outline_components());
    let mut host = Host::start(runs.clone());
    let answer = host.call("describe_page", json!({ "run_id": RUN_ID }));
    assert_eq!(answer["isError"], false, "{answer}");
    assert_eq!(
        groups_of(&answer),
        [
            "banner",
            "main",
            "contentinfo",
            "list \"x\"",
            "(page)",
            "region \"a > b \\\" c\"",
            "frame \"pay\" > main"
        ]
    );
    let body = text_of(&answer);
    assert_eq!(body["total"], 9);
    assert_eq!(body["untrusted_page_content"], true);
    let main = &body["groups"][1];
    assert_eq!(main["count"], 2);
    let templated = &main["items"][0];
    assert_eq!(templated["name"], "*");
    assert_eq!(templated["instances"], 5);
    assert_eq!(templated["example_names"], json!(["Item 1", "Item 2", "Item 3"]));
    assert!(main["items"][1].get("example_names").is_none());

    let narrow = text_of(&host.call("describe_page", json!({ "run_id": RUN_ID, "max_per_group": 1 })));
    assert_eq!(narrow["groups"][0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(narrow["groups"][0]["more"], 1);
    assert!(narrow["groups"][1].get("more").is_none() || narrow["groups"][1]["more"] == 1);
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn describe_page_concise_and_detailed_differ_and_an_unknown_format_is_refused() {
    let runs = runs_folder("describe-format");
    set_components(&runs, "firefox-1280x800", outline_components());
    let mut host = Host::start(runs.clone());
    let concise = text_of(&host.call("describe_page", json!({ "run_id": RUN_ID })));
    let detailed = text_of(&host.call(
        "describe_page",
        json!({ "run_id": RUN_ID, "response_format": "detailed" }),
    ));
    assert!(concise["groups"][0]["items"][0].get("path").is_none());
    assert!(detailed["groups"][0]["items"][0]["path"].is_string());
    assert!(concise.to_string().len() < detailed.to_string().len());
    let refused = host.call("describe_page", json!({ "run_id": RUN_ID, "response_format": "xml" }));
    assert_eq!(refused["isError"], true);
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

fn two_browser_runs(name: &str) -> PathBuf {
    let runs = runs_folder(name);
    let mut firefox = vec![
        component(
            "c_00000000000a",
            "button",
            "Add to cart",
            "main",
            &["Add to cart", "Add to cart"],
        ),
        component("c_00000000000b", "link", "Menu", "banner", &["Menu"]),
    ];
    firefox[1]["instances"][0]["attrs"] = json!({ "href": "/Cart-page", "data-testid": "menu" });
    set_components(&runs, "firefox-1280x800", firefox);
    let mut chromium = vec![component(
        "c_00000000000a",
        "button",
        "Add to cart",
        "main",
        &["Add to cart"],
    )];
    chromium.push(component("c_00000000000c", "checkbox", "Gift", "main", &["Gift"]));
    chromium[1]["instances"][0]["state"] =
        json!({ "visible": true, "enabled": true, "inViewport": true, "checked": true });
    set_components(&runs, "chromium-1280x800", chromium);
    runs
}

#[test]
fn find_answers_which_browsers_have_a_control() {
    let runs = two_browser_runs("find");
    let mut host = Host::start(runs.clone());
    let mut rows = |args: Value| -> Vec<Value> {
        let answer = host.call("find", args);
        assert_eq!(answer["isError"], false, "{answer}");
        text_of(&answer)["results"].as_array().unwrap().clone()
    };
    let by_name = rows(json!({ "run_id": RUN_ID, "name": "add to" }));
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0]["id"], "c_00000000000a");
    assert_eq!(by_name[0]["missing_in"], json!([]));
    let found: Vec<(&str, u64)> = by_name[0]["found_in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["target"].as_str().unwrap(), f["instances"].as_u64().unwrap()))
        .collect();
    assert_eq!(found, [("chromium-1280x800", 1), ("firefox-1280x800", 2)]);

    // The menu link exists only in firefox, found through its href, ignoring case.
    let menu = rows(json!({ "run_id": RUN_ID, "query": "cart-PAGE" }));
    assert_eq!(menu.len(), 1);
    assert_eq!(menu[0]["found_in"][0]["target"], "firefox-1280x800");
    assert_eq!(menu[0]["missing_in"], json!(["chromium-1280x800"]));
    assert_eq!(
        rows(json!({ "run_id": RUN_ID, "query": "menu" })).len(),
        1,
        "data-testid"
    );

    assert_eq!(
        rows(json!({ "run_id": RUN_ID, "role": "checkbox" }))[0]["id"],
        "c_00000000000c"
    );
    assert_eq!(rows(json!({ "run_id": RUN_ID, "state": "checked" })).len(), 1);
    assert_eq!(rows(json!({ "run_id": RUN_ID, "state": "unchecked" })).len(), 0);
    assert_eq!(rows(json!({ "run_id": RUN_ID, "state": "visible" })).len(), 3);
    assert_eq!(rows(json!({ "run_id": RUN_ID, "state": "hidden" })).len(), 0);
    // One target only: nothing is "missing" from a target that was not searched.
    let one = rows(json!({ "run_id": RUN_ID, "target": "firefox-1280x800", "role": "button" }));
    assert_eq!(one[0]["missing_in"], json!([]));
    assert_eq!(one[0]["found_in"].as_array().unwrap().len(), 1);

    let bad = host.call("find", json!({ "run_id": RUN_ID, "state": "sparkly" }));
    assert_eq!(bad["isError"], true);
    assert!(bad["content"][0]["text"].as_str().unwrap().contains("visible"));
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn concise_rows_are_short_and_detailed_rows_are_todays_fields() {
    let runs = runs_folder("formats");
    let mut host = Host::start(runs.clone());
    let concise = text_of(&host.call("get_components", json!({ "run_id": RUN_ID })));
    let row = concise["components"][0].as_object().unwrap();
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "instances", "name", "role"]);
    let detailed = text_of(&host.call(
        "get_components",
        json!({ "run_id": RUN_ID, "response_format": "detailed" }),
    ));
    for key in [
        "id",
        "role",
        "name",
        "instances",
        "visible",
        "path",
        "selector",
        "confidence",
        "has_crop",
    ] {
        assert!(detailed["components"][0].get(key).is_some(), "{key}");
    }
    // hidden and template appear only when true.
    change_components(&runs, |file| {
        file["components"][0]["name"] = json!("*");
        file["components"][0]["nameIsTemplate"] = json!(true);
        for i in file["components"][0]["instances"].as_array_mut().unwrap() {
            i["state"]["visible"] = json!(false);
        }
    });
    let flagged = text_of(&host.call("get_components", json!({ "run_id": RUN_ID })));
    assert_eq!(flagged["components"][0]["hidden"], true);
    assert_eq!(flagged["components"][0]["template"], true);
    let bad = host.call("get_components", json!({ "run_id": RUN_ID, "response_format": "tiny" }));
    assert_eq!(bad["isError"], true);
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn a_page_of_results_names_the_next_offset_and_how_to_narrow() {
    let runs = runs_folder("paging");
    let many: Vec<Value> = (0..30)
        .map(|n| component(&format!("c_{n:012}"), "button", &format!("Button {n}"), "main", &["x"]))
        .collect();
    set_components(&runs, "firefox-1280x800", many);
    let mut host = Host::start(runs.clone());
    let first = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "limit": 10 })));
    assert_eq!(first["shown"], 10);
    assert_eq!(first["next_offset"], 10);
    assert_eq!(
        first["note"].as_str().unwrap(),
        "Showing 10 of 30. Narrow with role (for example button), name (substring) or landmark, \
         or continue with offset=10."
    );
    assert!(first.get("truncated").is_none(), "truncated is for the 100 KB cut");
    let last = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "limit": 10, "offset": 20 })));
    assert_eq!(last["components"][0]["id"], "c_000000000020");
    assert!(last.get("next_offset").is_none() && last.get("note").is_none());
    let found = text_of(&host.call("find", json!({ "run_id": RUN_ID, "limit": 5, "offset": 5 })));
    assert_eq!(found["results"][0]["id"], "c_000000000005");
    assert_eq!(found["next_offset"], 10);
    assert!(found["note"].as_str().unwrap().contains("offset=10"));
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn modern_hosts_get_output_schemas_and_structured_content_that_matches_the_text() {
    let runs = two_browser_runs("structured");
    let mut host = Host::start(runs.clone());
    hello(&mut host, "2025-06-18");
    let tools = host.request("tools/list", json!({}))["result"]["tools"].clone();
    for tool in tools.as_array().unwrap() {
        assert_eq!(tool["outputSchema"]["type"], "object", "{}", tool["name"]);
    }
    let calls = [
        ("list_runs", json!({})),
        (
            "get_components",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800" }),
        ),
        (
            "get_components",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800", "component_id": "c_00000000000a" }),
        ),
        (
            "describe_page",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800" }),
        ),
        ("find", json!({ "run_id": RUN_ID })),
        ("extract", json!({ "url": "https://example.test/" })),
    ];
    for (tool, arguments) in calls {
        let result = host.call(tool, arguments);
        assert_eq!(result["isError"], false, "{tool}: {result}");
        assert_eq!(result["structuredContent"], text_of(&result), "{tool}");
        assert_eq!(result["structuredContent"]["untrusted_page_content"], true, "{tool}");
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not instructions"));
    }
    let crop = host.call(
        "get_crop",
        json!({ "run_id": RUN_ID, "target": "firefox-1280x800", "component_id": "c_00000000000a" }),
    );
    // No picture was taken for this one: an error carries no structured content.
    assert_eq!(crop["isError"], true);
    assert!(crop.get("structuredContent").is_none());
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn get_crop_and_extract_point_at_resources_for_modern_hosts_only() {
    let runs = runs_folder("links");
    let mut host = Host::start(runs.clone());
    hello(&mut host, "2025-06-18");
    let crop = host.call("get_crop", json!({ "run_id": RUN_ID, "component_id": COMPONENT }));
    assert_eq!(crop["content"][1]["type"], "image", "the picture stays where it was");
    let link = &crop["content"][2];
    assert_eq!(link["type"], "resource_link");
    assert_eq!(
        link["uri"],
        format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/{COMPONENT}")
    );
    assert_eq!(link["mimeType"], "image/png");
    assert_eq!(crop["structuredContent"]["component_id"], COMPONENT);
    let extract = host.call("extract", json!({ "url": "https://example.test/" }));
    let uris: Vec<&str> = extract["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "resource_link")
        .map(|b| b["uri"].as_str().unwrap())
        .collect();
    assert!(uris.iter().all(|u| u.starts_with("traceqa://runs/r_")), "{uris:?}");
    assert!(!uris.is_empty());
    assert_eq!(extract["content"].as_array().unwrap().last().unwrap()["type"], "text");
    host.stop();

    let mut old = Host::start(runs.clone());
    hello(&mut old, "2025-03-26");
    let tools = old.request("tools/list", json!({}))["result"]["tools"].clone();
    assert!(tools
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t.get("outputSchema").is_none()));
    for result in [
        old.call("get_crop", json!({ "run_id": RUN_ID, "component_id": COMPONENT })),
        old.call("get_components", json!({ "run_id": RUN_ID })),
        old.call("extract", json!({ "url": "https://example.test/" })),
    ] {
        assert!(result.get("structuredContent").is_none(), "{result}");
        assert!(
            result["content"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| b["type"] != "resource_link"),
            "{result}"
        );
    }
    old.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn hostile_names_are_gone_from_text_and_structured_content_of_every_new_answer() {
    let runs = two_browser_runs("hostile");
    let evil = "Buy\u{E0049}\u{202E} now\u{200B}\u{2066}";
    for dir in ["firefox-1280x800", "chromium-1280x800"] {
        let path = runs.join(RUN_ID).join(dir).join("components.json");
        let mut file: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let first = &mut file["components"][0];
        first["name"] = json!(evil);
        first["path"] = json!(format!("region \"{evil}\" > button"));
        first["instances"][0]["name"] = json!(evil);
        first["instances"][0]["attrs"] = json!({ format!("data-{evil}"): evil, "href": evil });
        fs::write(path, file.to_string()).unwrap();
    }
    let mut host = Host::start(runs.clone());
    hello(&mut host, "2025-11-25");
    let results = [
        host.call(
            "describe_page",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800" }),
        ),
        host.call(
            "describe_page",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800", "response_format": "detailed" }),
        ),
        host.call(
            "find",
            json!({ "run_id": RUN_ID, "query": "buy", "response_format": "detailed" }),
        ),
        host.call(
            "get_components",
            json!({ "run_id": RUN_ID, "target": "firefox-1280x800" }),
        ),
    ];
    for result in results {
        assert_eq!(result["isError"], false, "{result}");
        assert!(result["structuredContent"].is_object());
        let everything = result.to_string();
        for bad in ["\u{202E}", "\u{200B}", "\u{2066}", "\u{E0049}"] {
            assert!(!everything.contains(bad), "{bad:?} in {everything}");
        }
        assert!(everything.contains("Buy now"), "{everything}");
    }
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn landmark_narrows_get_components_and_find() {
    let runs = runs_folder("landmark");
    set_components(&runs, "firefox-1280x800", outline_components());
    let mut host = Host::start(runs.clone());
    let list = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "landmark": "MAIN" })));
    // main (2 components) and `frame "pay" > main` (1).
    assert_eq!(list["matching"], 3);
    let found = text_of(&host.call(
        "find",
        json!({ "run_id": RUN_ID, "landmark": "banner", "role": "link" }),
    ));
    assert_eq!(found["matching"], 1);
    assert_eq!(found["results"][0]["id"], "c_000000000001");
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

fn read(host: &mut Host, uri: &str) -> Value {
    host.request("resources/read", json!({ "uri": uri }))
}

#[test]
fn resources_list_templates_and_read() {
    let runs = runs_folder("resources");
    let mut host = Host::start(runs.clone());
    let init = host.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    assert!(init["result"]["capabilities"]["resources"].is_object());

    let listed = host.request("resources/list", json!({}))["result"]["resources"].clone();
    assert_eq!(listed[0]["uri"], format!("traceqa://runs/{RUN_ID}"));
    assert_eq!(listed[0]["mimeType"], "application/json");
    assert_eq!(listed[0]["annotations"]["audience"], json!(["assistant"]));

    let templates = host.request("resources/templates/list", json!({}))["result"]["resourceTemplates"].clone();
    let uris: Vec<&str> = templates
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["uriTemplate"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "traceqa://runs/{run_id}",
            "traceqa://runs/{run_id}/{target}/components",
            "traceqa://runs/{run_id}/{target}/crops/{component_id}"
        ]
    );

    let run = read(&mut host, &format!("traceqa://runs/{RUN_ID}"));
    let content = &run["result"]["contents"][0];
    assert_eq!(content["mimeType"], "application/json");
    let body: Value = serde_json::from_str(content["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["runId"], RUN_ID);
    assert_eq!(body["untrusted_page_content"], true);

    let uri = format!("traceqa://runs/{RUN_ID}/firefox-1280x800/components");
    let components = read(&mut host, &uri);
    let body: Value = serde_json::from_str(components["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["components"][0]["id"], COMPONENT);
    assert_eq!(body["untrusted_page_content"], true);

    let uri = format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/{COMPONENT}");
    let crop = read(&mut host, &uri);
    let content = &crop["result"]["contents"][0];
    assert_eq!(content["mimeType"], "image/png");
    assert_eq!(content["blob"], STANDARD.encode(PICTURE));
    assert_eq!(content["uri"], uri);
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn resources_list_shows_only_the_newest_fifty_runs() {
    let runs = runs_folder("resources-cap");
    let template: Value =
        serde_json::from_str(&fs::read_to_string(runs.join(RUN_ID).join("run.json")).unwrap()).unwrap();
    for n in 0..60 {
        let id = format!("r_20270101_0000{n:02}_aaaa");
        let dir = runs.join(&id);
        fs::create_dir_all(&dir).unwrap();
        let mut manifest = template.clone();
        manifest["runId"] = json!(id);
        manifest["startedAt"] = json!(format!("2027-01-01T00:00:{n:02}Z"));
        fs::write(dir.join("run.json"), manifest.to_string()).unwrap();
    }
    let mut host = Host::start(runs.clone());
    let listed = host.request("resources/list", json!({}))["result"]["resources"].clone();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 50);
    assert_eq!(listed[0]["uri"], "traceqa://runs/r_20270101_000059_aaaa");
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn resources_read_refuses_escapes_links_and_unknown_things() {
    let runs = runs_folder("resources-refuse");
    let outside = std::env::temp_dir().join(format!("traceqa-mcp-res-{}", process::id()));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(outside.join("crops")).unwrap();
    fs::write(outside.join("crops").join(format!("{COMPONENT}.png")), SECRET).unwrap();
    fs::copy(
        Path::new(EXAMPLES).join("ComponentsFile.shop.json"),
        outside.join("components.json"),
    )
    .unwrap();
    let mut host = Host::start(runs.clone());

    for uri in [
        "traceqa://runs/..".to_string(),
        "traceqa://runs/../etc/x/components".to_string(),
        format!("traceqa://runs/{RUN_ID}/../firefox-1280x800/components"),
        format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/..%2f..%2frun"),
        format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/a\\b"),
        format!("traceqa://runs/{RUN_ID}/firefox-1280x800/other"),
        "file:///etc/passwd".to_string(),
        "traceqa://runs".to_string(),
    ] {
        let answer = read(&mut host, &uri);
        let code = answer["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("{uri}: {answer}"));
        assert!(code == -32602 || code == -32002, "{uri}: {answer}");
        assert!(!answer.to_string().contains("TOP-SECRET"));
    }
    let missing = read(&mut host, "traceqa://runs/r_nope");
    assert_eq!(missing["error"]["code"], -32002);
    let no_target = read(&mut host, &format!("traceqa://runs/{RUN_ID}/webkit-1x1/components"));
    assert_eq!(no_target["error"]["code"], -32002);
    let no_component = read(
        &mut host,
        &format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/c_nope"),
    );
    assert_eq!(no_component["error"]["code"], -32002);

    // A target folder that is a link out of the runs folder.
    let target = runs.join(RUN_ID).join("firefox-1280x800");
    fs::remove_dir_all(&target).unwrap();
    link_dir(&target, &outside);
    for tail in ["components".to_string(), format!("crops/{COMPONENT}")] {
        let answer = read(&mut host, &format!("traceqa://runs/{RUN_ID}/firefox-1280x800/{tail}"));
        assert!(answer["error"].is_object(), "{tail}: {answer}");
        let text = answer.to_string();
        assert!(
            !text.contains("TOP-SECRET") && !text.contains(&STANDARD.encode(SECRET)),
            "{text}"
        );
        assert!(!text.contains(runs.to_str().unwrap()), "{text}");
    }
    unlink_dir(&target);

    // A picture that is itself too large is refused.
    fs::create_dir_all(target.join("crops")).unwrap();
    fs::write(
        target.join("crops").join(format!("{COMPONENT}.png")),
        vec![0u8; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    fs::copy(
        Path::new(EXAMPLES).join("ComponentsFile.shop.json"),
        target.join("components.json"),
    )
    .unwrap();
    let big = read(
        &mut host,
        &format!("traceqa://runs/{RUN_ID}/firefox-1280x800/crops/{COMPONENT}"),
    );
    assert!(big["error"].is_object(), "{big}");
    host.stop();
    fs::remove_dir_all(runs).unwrap();
    fs::remove_dir_all(outside).unwrap();
}
