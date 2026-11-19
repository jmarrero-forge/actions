//! Rust owns the mint scenarios; a small Node bridge executes the shipped
//! helper with real Fetch Responses, without credentials or network access.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const BRIDGE: &str = r#"
import { KEY_EXPIRY_SECONDS, mint } from './tailscale/mint.mjs';
const {env, responses} = JSON.parse(process.env.MINT_CASE);
const calls = [];
const logs = [];
console.log = (...args) => logs.push(args.join(' '));
const deadline = setTimeout(() => process.exit(2), 5000);
let key = null, error = null;
try {
  key = await mint(env, async (url, options) => {
    calls.push({url: String(url), method: options.method ?? 'GET',
      headers: options.headers ?? {}, redirect: options.redirect ?? null,
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
process.stdout.write(JSON.stringify({key, error, calls, logs, expiry: KEY_EXPIRY_SECONDS}));
"#;

fn responses() -> Vec<Value> {
    vec![
        json!({"body": {"value": "identity-fixture"}}),
        json!({"body": {"scope": "auth_keys", "access_token": "access-fixture"}}),
        json!({"body": {"key": "tskey-auth-fixture"}}),
    ]
}

fn exercise(overrides: Value, responses: Vec<Value>) -> Value {
    let mut env = json!({
        "ACTIONS_ID_TOKEN_REQUEST_URL": "https://oidc.example/token?existing=1&audience=old",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-fixture",
        "AUDIENCE": "test-audience", "OAUTH_CLIENT_ID": "test-client",
        "TAGS": "tag:one, , tag:two"
    });
    env.as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    let output = Command::new("node")
        .args(["--input-type=module", "-e", BRIDGE])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .env(
            "MINT_CASE",
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
    serde_json::from_slice(&output.stdout).expect("mint bridge must return JSON")
}

#[test]
fn mint_requests_a_single_use_ephemeral_key_without_logging_credentials() {
    let result = exercise(json!({}), responses());
    assert_eq!(result["error"], Value::Null, "{result}");
    assert_eq!(result["key"], "tskey-auth-fixture");
    assert_eq!(result["expiry"], 3600);
    assert_eq!(result["logs"], json!([]));
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
    assert_eq!(
        calls[2]["url"],
        "https://api.tailscale.com/api/v2/tailnet/-/keys"
    );
    assert_eq!(calls[2]["method"], "POST");
    assert_eq!(calls[2]["redirect"], "error");
    assert_eq!(
        calls[2]["headers"],
        json!({"Authorization": "Bearer access-fixture", "Content-Type": "application/json"})
    );
    assert_eq!(
        serde_json::from_str::<Value>(calls[2]["body"].as_str().unwrap()).unwrap(),
        json!({
            "capabilities": {"devices": {"create": {
                "reusable": false, "ephemeral": true, "preauthorized": true,
                "tags": ["tag:one", "tag:two"]
            }}}, "expirySeconds": 3600
        })
    );
}

fn assert_failure(result: &Value, message: &str, calls: usize) {
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| error.contains(message)),
        "{result}"
    );
    assert_eq!(result["key"], Value::Null);
    assert_eq!(result["calls"].as_array().unwrap().len(), calls, "{result}");
    assert_eq!(result["logs"], json!([]));
}

#[test]
fn invalid_environment_is_refused_before_requesting_credentials() {
    for (env, message) in [
        (json!({"RUNNER_DEBUG": "1"}), "debug logging"),
        (json!({"RUNNER_DEBUG": "0"}), "debug logging"),
        (json!({"TAGS": " , "}), "tags"),
        (json!({"TAGS": null}), "tags"),
    ] {
        assert_failure(&exercise(env, vec![]), message, 0);
    }
}

#[test]
fn http_and_transport_failures_stop_at_each_stage() {
    for (stage, label, status) in [
        (0, "Identity request", 403),
        (1, "Token exchange", 401),
        (2, "Key creation", 403),
    ] {
        for (response, message) in [
            (
                json!({"status": status}),
                format!("{label} failed: HTTP {status}"),
            ),
            (
                json!({"error": "network unavailable"}),
                "network unavailable".into(),
            ),
            (json!({"raw": "not JSON"}), "JSON".into()),
        ] {
            let mut replies = responses();
            replies[stage] = response;
            assert_failure(&exercise(json!({}), replies), &message, stage + 1);
        }
    }
}

#[test]
fn missing_or_malformed_credentials_never_advance_to_the_next_stage() {
    for (stage, field, message) in [
        (0, "value", "did not include a token"),
        (1, "access_token", "access token"),
        (2, "key", "no auth key"),
    ] {
        let mut bodies = vec![json!({})];
        for value in [Value::Null, json!(false), json!(42), json!(""), json!([])] {
            bodies.push(json!({field: value}));
        }
        if stage == 2 {
            for key in [
                "tskey-a\nb=c",
                "not-a-key",
                "tskey-",
                "tskey-a\r",
                "tskey-a ",
            ] {
                bodies.push(json!({field: key}));
            }
        }
        for body in bodies {
            let mut replies = responses();
            replies[stage] = json!({"body": body});
            assert_failure(&exercise(json!({}), replies), message, stage + 1);
        }
    }
}
