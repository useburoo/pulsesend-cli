//! `pulsesend login` par le navigateur (flux « device » de la RFC 8628) : le CLI affiche un code,
//! ouvre la page /cli/activate, attend que la personne approuve, puis reçoit une clé API créée
//! pour cette machine (visible et révocable dans Clés API).

use crate::api;
use crate::ui::{self, Ui};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct Approved {
    pub key: String,
    pub key_name: String,
    pub mode: String,
    pub team: Option<String>,
}

pub fn run(ui: Ui, base: &str, json_output: bool) -> Result<Approved> {
    let start = match api::cli_post(
        base,
        "device",
        &json!({
            "client_name": hostname(),
            "client_os": os_name(),
            "client_version": env!("CARGO_PKG_VERSION"),
        }),
    ) {
        Ok((200, body)) => body,
        Ok((status, body)) => bail!("could not start the login (HTTP {status}): {}", message(&body)),
        Err(e) => return Err(e.into()),
    };

    let field = |name: &str| start.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let device_code = field("device_code");
    let user_code = field("user_code");
    let page = field("verification_uri");
    let page_with_code = field("verification_uri_complete");
    let mut interval = start.get("interval").and_then(Value::as_u64).unwrap_or(5).max(1);
    let expires_in = start.get("expires_in").and_then(Value::as_u64).unwrap_or(600);

    if device_code.is_empty() || user_code.is_empty() {
        bail!("unexpected response from the API: no login code");
    }

    // Les consignes vont sur stderr : en --json, stdout ne garde que le résultat.
    if !json_output {
        eprintln!(
            "{}  {}",
            ui::strong("PulseSend"),
            ui::label(concat!("v", env!("CARGO_PKG_VERSION")))
        );
        eprintln!();
        eprintln!("{}", ui::label("Open this page to connect the CLI:"));
        eprintln!("  {page}");
        eprintln!();
        eprintln!("{}", ui::label("and check that it shows this code:"));
        eprintln!("  {}", ui::strong(&user_code));
        eprintln!();
    } else {
        eprintln!("Open {page_with_code} and check the code {user_code}");
    }

    if open_browser(&page_with_code) {
        ui.ok("Browser opened", "");
    } else {
        ui.warn("Could not open a browser: open the page above yourself.");
    }

    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let step = ui.step("Waiting for you to approve in the browser");
    let mut failures = 0;

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            step.fail("The code expired", "run pulsesend login again");
            bail!("the login code expired before it was approved");
        }
        step.detail(&format!("{}:{:02} left", left.as_secs() / 60, left.as_secs() % 60));
        sleep(Duration::from_secs(interval));

        match api::cli_post(base, "token", &json!({ "device_code": device_code })) {
            Ok((200, body)) => {
                let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
                let Some(key) = text("api_key") else {
                    step.fail("Login failed", "no key in the answer");
                    bail!("unexpected response from the API: no API key");
                };
                let approved = Approved {
                    key,
                    key_name: text("key_name").unwrap_or_default(),
                    mode: text("mode").unwrap_or_else(|| "test".into()),
                    team: text("team"),
                };
                step.done(
                    "Approved",
                    &[approved.team.clone().unwrap_or_default(), approved.mode.clone()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
                return Ok(approved);
            }
            Ok((_, body)) => match body.get("error").and_then(Value::as_str) {
                Some("authorization_pending") => failures = 0,
                Some("slow_down") => interval += 5,
                Some("access_denied") => {
                    step.fail("Canceled", "the connection was refused in the browser");
                    bail!("the login was canceled in the browser");
                }
                Some("expired_token") => {
                    step.fail("The code expired", "run pulsesend login again");
                    bail!("the login code expired or was already used");
                }
                // Limite de débit, erreur serveur… : on réessaie jusqu'à l'expiration du code.
                _ => {
                    failures += 1;
                    if failures >= 5 {
                        step.fail("Login failed", &message(&body));
                        bail!("could not finish the login: {}", message(&body));
                    }
                }
            },
            Err(e) => {
                failures += 1;
                if failures >= 5 {
                    step.fail("Login failed", "");
                    return Err(e.into());
                }
            }
        }
    }
}

/// Le message d'une réponse d'erreur, au format PulseSend ou RFC 8628.
fn message(body: &Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.get("message"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("unexpected response")
        .to_string()
}

fn open_browser(url: &str) -> bool {
    if std::env::var_os("PULSESEND_NO_BROWSER").is_some() {
        return false;
    }

    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// Le nom de la machine, pour nommer la clé (« CLI · jiordi-laptop »).
fn hostname() -> String {
    let from_env = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    let from_file = || std::fs::read_to_string("/etc/hostname").ok();
    let from_command = || {
        Command::new("hostname")
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
    };

    from_env
        .or_else(from_file)
        .or_else(from_command)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
        .chars()
        .take(60)
        .collect()
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    }
}
