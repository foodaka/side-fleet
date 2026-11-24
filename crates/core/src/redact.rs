//! Redaction of anything that looks like a credential. Applied by the worker to
//! tool inputs, tool results and stderr before they leave the machine, and by
//! the control plane defensively to anything it stores.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

pub const REDACTED: &str = "[REDACTED]";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Anthropic, OpenAI and similar `sk-` keys.
        r"sk-[A-Za-z0-9_\-]{16,}",
        // GitHub tokens.
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        // Slack.
        r"xox[abprs]-[A-Za-z0-9\-]{10,}",
        // AWS access key ids.
        r"AKIA[0-9A-Z]{16}",
        // Google API keys.
        r"AIza[0-9A-Za-z_\-]{30,}",
        // JWTs.
        r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        // Private key blocks.
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("valid regex"))
    .collect()
});

/// `Authorization: Bearer xyz` and bare `Bearer xyz`.
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9_\-\.=+/]{8,}").unwrap());

/// `token=abc`, `api_key: "abc"`, `PASSWORD='abc'`, `--token abc` and friends.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:--?(?:api[_\-]?key|access[_\-]?token|auth[_\-]?token|client[_\-]?secret|password|passwd|secret|token)(?:[=\s]+)|(?:api[_\-]?key|access[_\-]?token|refresh[_\-]?token|auth[_\-]?token|client[_\-]?secret|secret[_\-]?key|private[_\-]?key|password|passwd|secret|token|authorization)["']?\s*[:=]\s*["']?))([^\s"',;&]{6,})"#,
    )
    .unwrap()
});

/// Credentials that are unlikely to be anything else, by name, for refusing to push
/// a file that holds one. Unlike [`redact`] this leaves out `key = value` shapes,
/// which code is full of.
static CREDENTIALS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("an API key (sk-…)", r"\bsk-[A-Za-z0-9_\-]{16,}"),
        ("a GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}"),
        ("a Slack token", r"\bxox[abprs]-[A-Za-z0-9\-]{10,}"),
        ("an AWS access key", r"\bAKIA[0-9A-Z]{16}\b"),
        ("a Google API key", r"\bAIza[0-9A-Za-z_\-]{30,}"),
        ("a Stripe secret key", r"\b[rs]k_live_[0-9A-Za-z]{16,}"),
        ("a private key", r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----"),
        ("an npm token", r"(?m)^\s*//\S*:_(?:authToken|auth|password)\s*=\s*\S"),
        ("a password in a URL", r"\b[a-z][a-z0-9+.\-]*://[^\s:/@\x22']+:[^\s:/@\x22'$<>{}]{6,}@[A-Za-z0-9.\-]+"),
    ]
    .iter()
    .map(|(what, p)| (*what, Regex::new(p).expect("valid regex")))
    .collect()
});

/// What kind of credential `text` holds, if it holds one of the unmistakable kinds.
pub fn find_credential(text: &str) -> Option<&'static str> {
    CREDENTIALS.iter().find(|(_, re)| re.is_match(text)).map(|(what, _)| *what)
}

/// Replace anything that looks like a credential with `[REDACTED]`.
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, REDACTED).into_owned();
    }
    out = BEARER
        .replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], REDACTED))
        .into_owned();
    out = KEY_VALUE
        .replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], REDACTED))
        .into_owned();
    out
}

/// Redact every string leaf of a JSON value, and replace the value of any key
/// whose name suggests a secret regardless of its content.
pub fn redact_json(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_json).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    if is_secret_key(k) {
                        (k.clone(), Value::String(REDACTED.into()))
                    } else {
                        (k.clone(), redact_json(v))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A tool call's input as it may leave the machine for an approval: redacted, with
/// every string clipped to `max_string` bytes (a `Write` can carry a whole file).
pub fn redact_tool_input(v: &Value, max_string: usize) -> Value {
    fn clip(v: Value, max: usize) -> Value {
        match v {
            Value::String(s) => Value::String(crate::stream::truncate_bytes(&s, max)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| clip(x, max)).collect()),
            Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, clip(x, max))).collect()),
            other => other,
        }
    }
    clip(redact_json(v), max_string)
}

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "authorization",
        "private_key",
        "credential",
    ]
    .iter()
    .any(|n| k.contains(n))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// A short, redacted, human-readable one-liner describing a tool call's input.
pub fn summarize_tool_input(tool: &str, input: &Value) -> String {
    let get = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let summary = match tool {
        "Bash" => get("command").unwrap_or_default(),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => get("file_path")
            .or_else(|| get("path"))
            .unwrap_or_default(),
        "Grep" | "Glob" => {
            let pat = get("pattern").unwrap_or_default();
            match get("path") {
                Some(p) => format!("{pat} in {p}"),
                None => pat,
            }
        }
        "WebFetch" => get("url").unwrap_or_default(),
        "WebSearch" => get("query").unwrap_or_default(),
        "Agent" | "Task" => get("description")
            .or_else(|| get("prompt"))
            .unwrap_or_default(),
        _ => serde_json::to_string(&redact_json(input)).unwrap_or_default(),
    };
    let summary = summary.replace('\n', " ⏎ ");
    truncate(&redact(&summary), 240)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unmistakable_credentials_but_not_code_about_them() {
        assert_eq!(find_credential("KEY=sk-ant-api03-abcdefghijklmnopqrstuv"), Some("an API key (sk-…)"));
        assert_eq!(find_credential("token: ghp_abcdefghijklmnopqrstuvwxyz0123"), Some("a GitHub token"));
        assert_eq!(find_credential("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"), Some("a private key"));
        assert_eq!(find_credential("//registry.npmjs.org/:_authToken=abc123"), Some("an npm token"));
        assert_eq!(find_credential("DATABASE_URL=postgres://app:hunter2hunter@db.internal:5432/app"), Some("a password in a URL"));
        assert_eq!(find_credential("let password = read_password(); // token = getToken()"), None);
        assert_eq!(find_credential("postgres://${USER}:${PASSWORD}@localhost/db"), None);
        assert_eq!(find_credential("see https://github.com/org/repo and task-sk-1"), None);
    }
    use serde_json::json;

    #[test]
    fn redacts_known_prefixes() {
        let s = "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123 and ghp_abcdefghijklmnopqrstuvwxyz1234";
        let r = redact(s);
        assert!(!r.contains("sk-ant"), "{r}");
        assert!(!r.contains("ghp_abc"), "{r}");
        assert_eq!(r.matches(REDACTED).count(), 2);
    }

    #[test]
    fn redacts_bearer_and_key_value_forms() {
        let r = redact("curl -H 'Authorization: Bearer abc123def456ghi' https://x");
        assert!(!r.contains("abc123def456ghi"), "{r}");
        let r = redact("export API_KEY=supersecretvalue123 && run");
        assert!(!r.contains("supersecretvalue123"), "{r}");
        assert!(r.contains("API_KEY="), "{r}");
        let r = redact("password: hunter2hunter2");
        assert!(!r.contains("hunter2"), "{r}");
        let r = redact("sidefleet run --token tok_abcdef123456 --control http://x");
        assert!(!r.contains("tok_abcdef"), "{r}");
        assert!(r.contains("--control http://x"), "{r}");
    }

    #[test]
    fn redacts_jwt_and_private_key() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(!redact(jwt).contains("eyJ"));
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nAAAA\n-----END RSA PRIVATE KEY-----";
        assert_eq!(redact(pem), REDACTED);
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let s = "git commit -m 'fix token parsing in lexer' && cargo test";
        assert_eq!(redact(s), s);
        let s = "Read the README and summarize it.";
        assert_eq!(redact(s), s);
        let sha = "commit 3f2a9c1e4b7d8e0f1a2b3c4d5e6f7a8b9c0d1e2f";
        assert_eq!(redact(sha), sha);
    }

    #[test]
    fn redacts_json_by_key_and_value() {
        let v = json!({"command": "echo sk-abcdefghijklmnopqrstuvwxyz", "api_key": "x", "nested": {"token": "y", "note": "ok"}});
        let r = redact_json(&v);
        assert_eq!(r["api_key"], REDACTED);
        assert_eq!(r["nested"]["token"], REDACTED);
        assert_eq!(r["nested"]["note"], "ok");
        assert!(!r["command"].as_str().unwrap().contains("sk-abc"));
    }

    #[test]
    fn tool_summary_is_short_and_redacted() {
        let s = summarize_tool_input(
            "Bash",
            &json!({"command": "curl -H 'Authorization: Bearer secrettoken123456' https://api.example.com"}),
        );
        assert!(!s.contains("secrettoken"), "{s}");
        assert!(s.starts_with("curl -H"), "{s}");
        let s = summarize_tool_input("Read", &json!({"file_path": "/repo/src/main.rs"}));
        assert_eq!(s, "/repo/src/main.rs");
        let s = summarize_tool_input("Custom", &json!({"token": "abc", "x": 1}));
        assert!(s.contains(REDACTED) && s.contains("\"x\":1"), "{s}");
        let long = "a".repeat(1000);
        let s = summarize_tool_input("Bash", &json!({"command": long}));
        assert!(s.chars().count() <= 241);
    }

    #[test]
    fn tool_input_for_approval_is_redacted_and_clipped() {
        let v = json!({"command": "export GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789", "content": "x".repeat(10_000), "n": 3});
        let r = redact_tool_input(&v, 100);
        assert!(!r["command"].as_str().unwrap().contains("ghp_abc"), "{r}");
        assert!(r["content"].as_str().unwrap().len() < 200);
        assert_eq!(r["n"], 3);
    }
}
