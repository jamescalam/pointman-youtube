//! extension.toml within what core's manifest check takes (Pointman's src/messageboard/extensions/manifest.py), and
//! its schemas as proto/youtube/v1/youtube.proto says them: each command's params are its `<Verb>Params`, the settings
//! `Settings`, field for field, each field's comment its description.

use std::collections::BTreeMap;

use regex::Regex;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn manifest() -> toml::Table {
    toml::from_str(&std::fs::read_to_string(format!("{ROOT}/extension.toml")).unwrap()).unwrap()
}

/// Each message's fields, with the comment above each: `{message: {field: comment}}`.
fn proto() -> BTreeMap<String, BTreeMap<String, String>> {
    let text = std::fs::read_to_string(format!("{ROOT}/proto/youtube/v1/youtube.proto")).unwrap();
    let message = Regex::new(r"^message (\w+) \{").unwrap();
    let field = Regex::new(r"^\s+(?:optional |repeated )?[\w.]+ (\w+) = \d+").unwrap();
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let (mut current, mut comment) = (String::new(), Vec::<String>::new());
    for line in text.lines() {
        if let Some(m) = message.captures(line) {
            current = m[1].to_string();
            out.entry(current.clone()).or_default();
            if line.trim_end().ends_with("{}") {
                current.clear();
            }
        } else if line.starts_with('}') {
            current.clear();
        } else if let Some(c) = line.trim().strip_prefix("// ") {
            comment.push(c.to_string());
            continue;
        } else if let Some(f) = field.captures(line).filter(|_| !current.is_empty()) {
            out.get_mut(&current).unwrap().insert(f[1].to_string(), comment.join(" "));
        }
        comment.clear();
    }
    out
}

fn pascal(verb: &str) -> String {
    verb.split('_').map(|w| w[..1].to_uppercase() + &w[1..]).collect()
}

/// A schema's properties and their descriptions.
fn properties(schema: &toml::Value) -> BTreeMap<String, String> {
    schema
        .get("properties")
        .and_then(|p| p.as_table())
        .map(|p| {
            p.iter()
                .map(|(k, v)| (k.clone(), v.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_manifest_keeps_to_cores_limits() {
    let m = manifest();
    let chars = |v: &toml::Value| v.as_str().unwrap().chars().count();
    let ext = &m["extension"];
    assert_eq!(ext["version"].as_str(), Some(env!("CARGO_PKG_VERSION")), "extension.toml's version is Cargo's");
    assert!((1..=60).contains(&chars(&ext["name"])) && (1..=200).contains(&chars(&ext["summary"])));
    assert_eq!(m["run"]["command"].as_array().unwrap()[0].as_str(), Some("bin/youtube"), "a binary runs as bin/<id>");
    for f in ["SKILL.md", "references/packaging.md", "references/thumbnails.md", "scripts/thumbs.py"] {
        assert!(std::path::Path::new(ROOT).join(f).is_file(), "{f} ships with the behaviour");
    }
    let slash = Regex::new("^[a-z][a-z0-9-]{0,23}$").unwrap();
    for c in m["commands"].as_array().unwrap() {
        let kind = c["kind"].as_str().unwrap();
        assert!(kind.starts_with("youtube."), "{kind}");
        assert!(chars(&c["title"]) <= 60, "{kind}'s title is over 60");
        assert!(chars(&c["summary"]) <= 120, "{kind}'s summary is over 120");
        assert!(slash.is_match(c["slash"].as_str().unwrap()), "{kind}'s slash");
    }
    let forms: Vec<&str> = m["credentials"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(forms, ["client", "account"]);
    for c in m["credentials"].as_array().unwrap() {
        let value = Regex::new(&format!("^(?:{})$", c["value"].as_str().unwrap())).unwrap();
        let example = match c["id"].as_str().unwrap() {
            "client" => "123-abc.apps.googleusercontent.com GOCSPX-abcdefghijklmnop",
            _ => "1//0gABCDEFGHIJKLMNOPQRSTUVWXYZ-_abcdef",
        };
        assert!(value.is_match(example), "{} takes {example}", c["id"]);
    }
}

#[test]
fn the_schemas_are_the_protos() {
    let m = manifest();
    let proto = proto();
    for c in m["commands"].as_array().unwrap() {
        let kind = c["kind"].as_str().unwrap();
        let message = format!("{}Params", pascal(kind.trim_start_matches("youtube.")));
        let fields = proto.get(&message).unwrap_or_else(|| panic!("{kind}: no message {message} in the proto"));
        let params = &c["params"];
        assert_eq!(params["type"].as_str(), Some("object"));
        assert_eq!(params["additionalProperties"].as_bool(), Some(false), "{kind}: unknown params are refused");
        assert_eq!(&properties(params), fields, "{kind}'s params are {message}'s fields, with their comments");
        let result = format!("{}Result", pascal(kind.trim_start_matches("youtube.")));
        assert!(proto.contains_key(&result), "{kind}: no message {result}");
        for r in params.get("required").and_then(|r| r.as_array()).into_iter().flatten() {
            assert!(fields.contains_key(r.as_str().unwrap()), "{kind}: {r} is required but isn't a field");
        }
    }
    assert_eq!(&properties(&m["settings"]), &proto["Settings"], "the settings are Settings' fields");
    let privacy = ["private", "unlisted", "public"];
    for c in m["commands"].as_array().unwrap() {
        if let Some(p) = c["params"].get("properties").and_then(|p| p.get("privacy")) {
            let words: Vec<&str> = p["enum"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
            assert_eq!(words, privacy, "Privacy's values, lowercase without PRIVACY_");
        }
    }
}
