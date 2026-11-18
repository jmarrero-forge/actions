//! Rust owns the scenarios and assertions; the bridge calls the shipped probe
//! with queued responses. No network, proxy, root or credentials are needed.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const ROOT: &str = "https://example.com/";
const AUDIT: &str = "https://registry.npmjs.org/-/npm/v1/security/advisories/bulk";
const DENIED: &str = "POST to an endpoint not in the write allowlist";

const BRIDGE: &str = r#"
import { probe } from './secure-host/egress-probe.mjs';
const {responses, env, allowed} = JSON.parse(process.env.PROBE_CASE);
const calls = [];
const logs = [];
console.log = (...args) => logs.push(args.join(' '));
const deadline = setTimeout(() => process.exit(2), 5000);
let error = null;
try {
  await probe(async (url, options) => {
    const call = {url, method: options.method, redirect: options.redirect,
      signal: options.signal instanceof AbortSignal, body: options.body ?? null,
      cancelled: false};
    calls.push(call);
    const response = responses.shift();
    if (!response) throw new Error('unexpected request');
    if (response.error) throw new Error(response.error);
    return {status: response.status, headers: new Headers(response.headers),
      body: {cancel: async () => { call.cancelled = true; }}};
  }, env, allowed);
} catch (e) { error = e.message; }
clearTimeout(deadline);
process.stdout.write(JSON.stringify({calls, logs, error}));
"#;

fn response(status: u16, denied: Option<&str>) -> Value {
    let headers = denied.map_or_else(|| json!({}), |value| json!({"x-egress-denied": value}));
    json!({"status": status, "headers": headers})
}

fn exercise(responses: Vec<Value>, allowed: &str, env: Value) -> Value {
    let output = Command::new("node")
        .args(["--input-type=module", "-e", BRIDGE])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .env(
            "PROBE_CASE",
            json!({"responses": responses, "allowed": allowed, "env": env}).to_string(),
        )
        .output()
        .expect("Node must be on PATH");
    assert!(
        output.status.success(),
        "probe bridge: {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("bridge must return JSON")
}

fn check_calls(result: &Value, expected: &[(&str, &str)], failed_request: bool) {
    let calls = result["calls"].as_array().unwrap();
    assert_eq!(calls.len(), expected.len(), "{result}");
    for (index, (call, (method, url))) in calls.iter().zip(expected).enumerate() {
        assert_eq!(call["url"], *url);
        assert_eq!(call["method"], *method);
        assert_eq!(call["redirect"], "error");
        assert_eq!(call["signal"], true);
        assert_eq!(
            call["body"],
            if *method == "POST" {
                json!("probe")
            } else {
                Value::Null
            }
        );
        assert_eq!(
            call["cancelled"],
            !(failed_request && index + 1 == calls.len())
        );
    }
}

#[test]
fn default_policy_requires_read_proxy_refusal_and_forwarded_audit() {
    let expected = [("GET", ROOT), ("POST", ROOT), ("POST", AUDIT)];
    for (name, replacement, index, count, success, network_error) in [
        ("proxy", response(403, None), 2, 3, true, false),
        ("origin", response(403, None), 1, 2, false, false),
        ("allowed", response(200, None), 1, 2, false, false),
        (
            "offline",
            json!({"error": "network unavailable"}),
            0,
            1,
            false,
            true,
        ),
        ("read denied", response(403, None), 0, 1, false, false),
        (
            "default refused",
            response(403, Some("refused")),
            2,
            3,
            false,
            false,
        ),
        (
            "default unforwarded",
            response(502, None),
            2,
            3,
            false,
            false,
        ),
        (
            "wrong denial",
            response(403, Some("unrelated refusal")),
            1,
            2,
            false,
            false,
        ),
    ] {
        let mut responses = vec![
            response(200, None),
            response(403, Some(DENIED)),
            response(403, None),
        ];
        responses[index] = replacement;
        let result = exercise(responses, "", json!({}));
        assert_eq!(result["error"].is_null(), success, "{name}: {result}");
        check_calls(&result, &expected[..count], network_error);
        if success {
            assert_eq!(
                result["logs"],
                json!([
                    "GET: allowed",
                    "POST: refused by proxy",
                    "npm audit POST: allowed by default policy"
                ])
            );
        }
    }
}

#[test]
fn replacement_policy_allows_only_its_rule_not_default_writes() {
    let expected = [
        ("GET", ROOT),
        ("POST", ROOT),
        ("POST", "https://example.com/allowed"),
        ("POST", AUDIT),
    ];
    for (name, replacement, index, count, success, network_error) in [
        (
            "rule origin refusal",
            response(403, None),
            2,
            4,
            true,
            false,
        ),
        ("rule success", response(200, None), 2, 4, true, false),
        (
            "rule refused",
            response(403, Some(DENIED)),
            2,
            3,
            false,
            false,
        ),
        ("all open", response(403, None), 1, 2, false, false),
        ("unforwarded", response(502, None), 2, 3, false, false),
        ("merged", response(200, None), 3, 4, false, false),
        (
            "default origin refused",
            response(403, None),
            3,
            4,
            false,
            false,
        ),
        (
            "default offline",
            json!({"error": "network unavailable"}),
            3,
            4,
            false,
            true,
        ),
        (
            "default wrong denial",
            response(403, Some("unrelated refusal")),
            3,
            4,
            false,
            false,
        ),
    ] {
        let mut responses = vec![
            response(200, None),
            response(403, Some(DENIED)),
            response(403, None),
            response(403, Some(DENIED)),
        ];
        responses[index] = replacement;
        let result = exercise(responses, "/allowed", json!({}));
        assert_eq!(result["error"].is_null(), success, "{name}: {result}");
        check_calls(&result, &expected[..count], network_error);
        if success {
            assert_eq!(
                result["logs"][3],
                "npm audit POST: refused by replacement policy"
            );
        }
    }
}

#[test]
fn explicit_proxy_environment_is_rejected_before_any_request() {
    let result = exercise(vec![], "", json!({"NODE_USE_ENV_PROXY": "1"}));
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("NODE_USE_ENV_PROXY must be unset")
    );
    assert_eq!(result["calls"], json!([]));
    assert_eq!(result["logs"], json!([]));
}
