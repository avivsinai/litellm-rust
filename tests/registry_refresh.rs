//! Regression test for scripts/update-model-registry.sh's high-water shrink
//! guard (issue #70): upstream dropped 21 xAI models that our trusted data
//! already marked deprecated, taking xai from 78 to 58 entries, and the
//! guard rejected every daily refresh. Reviewed retirements must lower the
//! baseline; the same shrink without past deprecation dates must still fail.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Map, Value};

/// A registry that passes validate-model-registry.sh: >= 1000 entries, every
/// core provider present, >= 30 output-config entries, and 78 xai entries of
/// which the first 20 carry `xai_deprecation` when given.
fn registry(xai_deprecation: Option<&str>) -> Map<String, Value> {
    let mut entries = Map::new();
    entries.insert("sample_spec".into(), json!({}));
    for provider in ["openai", "anthropic", "gemini", "zai", "openrouter"] {
        entries.insert(
            format!("{provider}/model"),
            json!({"litellm_provider": provider, "mode": "chat"}),
        );
    }
    for i in 0..1000 {
        entries.insert(
            format!("other/model-{i}"),
            json!({"litellm_provider": "other", "mode": "chat", "supports_output_config": i < 40}),
        );
    }
    for i in 0..78 {
        let mut entry = json!({"litellm_provider": "xai", "mode": "chat"});
        if let (true, Some(date)) = (i < 20, xai_deprecation) {
            entry["deprecation_date"] = json!(date);
        }
        entries.insert(format!("xai/grok-{i}"), entry);
    }
    entries
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// Runs the refresh with `trusted` committed, an xai high-water of 78, and
/// upstream equal to `trusted` minus the first 20 xai entries (78 -> 58).
fn refresh_after_xai_removal(case: &str, trusted: Map<String, Value>) -> (Output, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("registry_refresh_{case}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut upstream = trusted.clone();
    for i in 0..20 {
        upstream.remove(&format!("xai/grok-{i}"));
    }
    let destination = dir.join("registry.json");
    let source = dir.join("upstream.json");
    let baseline = dir.join("baseline.json");
    write_json(&destination, &Value::Object(trusted));
    write_json(&source, &Value::Object(upstream));
    write_json(
        &baseline,
        &json!({"total": 1083, "core_providers": {
            "openai": 1, "anthropic": 1, "gemini": 1, "xai": 78, "zai": 1, "openrouter": 1}}),
    );

    let output = Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/update-model-registry.sh"))
        .env("TMPDIR", &dir)
        .env(
            "MODEL_REGISTRY_SOURCE_URL",
            format!("file://{}", source.display()),
        )
        .env("MODEL_REGISTRY_DESTINATION", &destination)
        .env("MODEL_REGISTRY_BASELINE", &baseline)
        .output()
        .unwrap();
    (output, baseline)
}

#[test]
fn shrink_guard_allows_only_reviewed_retirements() {
    let (output, baseline) = refresh_after_xai_removal("retired", registry(Some("2000-01-01")));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let baseline: Value = serde_json::from_slice(&std::fs::read(baseline).unwrap()).unwrap();
    assert_eq!(baseline["core_providers"]["xai"], 58);

    let (output, _) = refresh_after_xai_removal("unexplained", registry(None));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("xai 58 vs high-water 78"));
}
