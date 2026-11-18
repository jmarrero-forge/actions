//! Rust owns the scenarios; Node executes the shipped helper with synthetic
//! Fetch responses. No credentials, tailnet, or network access are needed.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const BRIDGE: &str = r#"
import { check } from './tailscale/check.mjs';
const {env, responses} = JSON.parse(process.env.CHECK_CASE);
const calls = [], logs = [];
console.log = (...args) => logs.push(args.join(' '));
const deadline = setTimeout(() => process.exit(2), 5000);
let error = null;
try {
  await check(env, async (url, options) => {
    calls.push({url: String(url), method: options.method ?? 'GET',
      headers: options.headers ?? null, redirect: options.redirect ?? null,
      signal: options.signal instanceof AbortSignal, aborted: options.signal?.aborted,
      body: options.body instanceof URLSearchParams
        ? Object.fromEntries(options.body) : options.body ?? null});
    const response = responses.shift();
    if (!response) throw new Error('unexpected request');
    if (response.error) throw new Error(response.error);
    return new Response(response.raw ?? JSON.stringify(response.body),
      {status: response.status ?? 200, headers: {'Content-Type': 'application/json'}});
  });
} catch (e) { error = e.message; }
clearTimeout(deadline);
process.stdout.write(JSON.stringify({error, calls, logs}));
"#;

fn responses() -> Vec<Value> {
    vec![
        json!({"body": {"value": "identity-fixture"}}),
        json!({"body": {"scope": "auth_keys", "access_token": "access-fixture"}}),
        json!({"status": 401}),
    ]
}

fn exercise(responses: Vec<Value>) -> Value {
    let env = json!({
        "ACTIONS_ID_TOKEN_REQUEST_URL": "https://oidc.example/token?existing=1&audience=old",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-fixture",
        "AUDIENCE": "test-audience", "OAUTH_CLIENT_ID": "test-client",
        "PROXY": "http://proxy.example:18080/ignored/path"
    });
    let output = Command::new("node")
        .args(["--input-type=module", "-e", BRIDGE])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .env(
            "CHECK_CASE",
            json!({"env": env, "responses": responses}).to_string(),
        )
        .output()
        .expect("Node must be on PATH");
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("check bridge must return JSON")
}

#[test]
fn reports_only_scopes_and_probes_without_credentials_or_redirects() {
    let result = exercise(responses());
    assert_eq!(result["error"], Value::Null, "{result}");
    assert_eq!(
        result["logs"],
        json!([
            "Granted scopes: auth_keys",
            "Proxy returned the expected unauthenticated HTTP 401"
        ])
    );
    let calls = result["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    for call in calls {
        assert_eq!(call["signal"], true);
        assert_eq!(call["aborted"], false);
    }
    assert_eq!(
        calls[0]["url"],
        "https://oidc.example/token?existing=1&audience=test-audience"
    );
    assert_eq!(calls[0]["method"], "GET");
    assert_eq!(
        calls[0]["headers"],
        json!({"Authorization": "Bearer request-fixture"})
    );
    assert_eq!(calls[0]["body"], Value::Null);
    assert_eq!(
        calls[1]["url"],
        "https://api.tailscale.com/api/v2/oauth/token-exchange"
    );
    assert_eq!(calls[1]["method"], "POST");
    assert_eq!(
        calls[1]["body"],
        json!({"client_id": "test-client", "jwt": "identity-fixture"})
    );
    assert_eq!(calls[2]["url"], "http://proxy.example:18080/v1/runs");
    assert_eq!(calls[2]["method"], "POST");
    assert_eq!(calls[2]["headers"], Value::Null);
    assert_eq!(calls[2]["body"], Value::Null);
    assert_eq!(calls[2]["redirect"], "error");
}

fn assert_failure(replies: Vec<Value>, message: &str, stage: usize) {
    let result = exercise(replies);
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|s| s.contains(message)),
        "{result}"
    );
    assert_eq!(
        result["calls"].as_array().unwrap().len(),
        stage + 1,
        "{result}"
    );
    assert_eq!(
        result["logs"],
        if stage == 2 {
            json!(["Granted scopes: auth_keys"])
        } else {
            json!([])
        }
    );
}

#[test]
fn failures_stop_at_each_stage_without_printing_response_bodies() {
    for stage in 0..3 {
        let mut cases = vec![(
            json!({"error": "network unavailable"}),
            "network unavailable",
        )];
        if stage < 2 {
            cases.extend([
                (
                    json!({"status": 403, "raw": "sensitive response"}),
                    "HTTP 403",
                ),
                (json!({"raw": "not JSON"}), "JSON"),
            ]);
        } else {
            for status in [200, 302, 403, 500] {
                cases.push((
                    json!({"status": status, "raw": "sensitive response"}),
                    "expected 401",
                ));
            }
        }
        for (response, message) in cases {
            let mut replies = responses();
            replies[stage] = response;
            assert_failure(replies, message, stage);
        }
    }
}

#[test]
fn missing_or_malformed_identity_and_scopes_are_refused() {
    for (stage, field, message) in [
        (0, "value", "did not include a token"),
        (1, "scope", "granted scopes"),
    ] {
        let mut bodies = vec![json!({})];
        for value in [Value::Null, json!(false), json!(42), json!([])] {
            bodies.push(json!({field: value}));
        }
        if stage == 0 {
            bodies.push(json!({"value": ""}));
        }
        for body in bodies {
            let mut replies = responses();
            replies[stage] = json!({"body": body});
            assert_failure(replies, message, stage);
        }
    }
}

#[test]
fn scope_reporting_does_not_require_an_access_token_or_nonempty_scope() {
    for scope in ["auth_keys", ""] {
        let mut replies = responses();
        replies[1] = json!({"body": {"scope": scope}});
        let result = exercise(replies);
        assert_eq!(result["error"], Value::Null, "{result}");
        assert_eq!(result["logs"][0], format!("Granted scopes: {scope}"));
        assert_eq!(result["calls"].as_array().unwrap().len(), 3);
    }
}
