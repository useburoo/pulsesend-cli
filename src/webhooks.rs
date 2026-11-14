//! pulsesend webhooks : endpoints qui reçoivent les événements des emails.
//! Clé avec le scope webhooks:read (lecture) ou webhooks:write (le reste).

use crate::ui::{self, Ui};
use crate::{Cli, Col, api, print_json, print_next, print_table, str_of};
use anyhow::{Result, bail};
use clap::Subcommand;
use serde_json::{Map, Value, json};
use std::process::ExitCode;

#[derive(Subcommand)]
pub enum WebhooksCommand {
    /// List your webhook endpoints.
    List {
        /// Endpoints per page (1 to 100).
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Cursor of the next page, printed at the end of the previous one.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Add an endpoint. Prints its signing secret, shown only once.
    Add {
        /// Public HTTPS URL that receives the events.
        #[arg(long)]
        url: String,
        /// Event to receive, e.g. email.delivered. Repeat or separate with commas. All events by default.
        #[arg(long = "event", value_delimiter = ',')]
        events: Vec<String>,
    },
    /// Show an endpoint.
    Get {
        /// Endpoint id (wh_…).
        id: String,
    },
    /// Change the URL or the events, or turn an endpoint on or off.
    Update {
        /// Endpoint id (wh_…).
        id: String,
        #[arg(long)]
        url: Option<String>,
        /// Events to receive, replacing the current ones. Repeat or separate with commas; "*" for all.
        #[arg(long = "event", value_delimiter = ',')]
        events: Vec<String>,
        /// Turn deliveries back on (also clears the failure count).
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Stop deliveries.
        #[arg(long)]
        disable: bool,
    },
    /// Replace the signing secret and print the new one.
    RotateSecret {
        /// Endpoint id (wh_…).
        id: String,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Send a webhook.test event to the endpoint.
    Test {
        /// Endpoint id (wh_…).
        id: String,
    },
    /// Delete an endpoint.
    Remove {
        /// Endpoint id (wh_…).
        id: String,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
}

pub fn run(cli: &Cli, ui: Ui, client: &api::Client, command: &WebhooksCommand) -> Result<ExitCode> {
    match command {
        WebhooksCommand::List { limit, cursor } => list(cli, ui, client, *limit, cursor.as_deref())?,
        WebhooksCommand::Add { url, events } => {
            let events = if events.is_empty() {
                vec!["*".to_string()]
            } else {
                trimmed(events)
            };
            let step = ui.step("Adding the endpoint");
            let endpoint = match client.post("webhooks", &json!({ "url": url, "events": events }), None) {
                Ok(e) => e,
                Err(e) => {
                    step.fail("Not added", "");
                    return Err(e.into());
                }
            };
            step.done("Endpoint added", &str_of(&endpoint, "id"));
            show(cli, ui, &endpoint);
        }
        WebhooksCommand::Get { id } => {
            let step = ui.step(&format!("Fetching {id}"));
            let endpoint = client.get(&format!("webhooks/{id}"), &[]);
            step.clear();
            show(cli, ui, &endpoint?);
        }
        WebhooksCommand::Update {
            id,
            url,
            events,
            enable,
            disable,
        } => {
            let mut changes = Map::new();
            if let Some(u) = url {
                changes.insert("url".into(), json!(u));
            }
            if !events.is_empty() {
                changes.insert("events".into(), json!(trimmed(events)));
            }
            if *enable || *disable {
                changes.insert("enabled".into(), json!(*enable));
            }
            if changes.is_empty() {
                bail!("nothing to change: pass --url, --event, --enable or --disable");
            }
            let step = ui.step(&format!("Updating {id}"));
            let endpoint = match client.patch(&format!("webhooks/{id}"), &Value::Object(changes)) {
                Ok(e) => e,
                Err(e) => {
                    step.fail("Not updated", "");
                    return Err(e.into());
                }
            };
            step.done("Endpoint updated", "");
            show(cli, ui, &endpoint);
        }
        WebhooksCommand::RotateSecret { id, yes } => {
            if !yes
                && !ui::confirm(&format!(
                    "Replace the secret of {id}? Signatures made with the old one stop matching."
                ))?
            {
                ui.warn("Secret unchanged.");
                return Ok(ExitCode::FAILURE);
            }
            let step = ui.step("Rotating the secret");
            let endpoint = match client.post(&format!("webhooks/{id}/rotate-secret"), &json!({}), None) {
                Ok(e) => e,
                Err(e) => {
                    step.fail("Not rotated", "");
                    return Err(e.into());
                }
            };
            step.done("New secret", "");
            show(cli, ui, &endpoint);
        }
        WebhooksCommand::Test { id } => {
            let step = ui.step(&format!("Sending webhook.test to {id}"));
            let sent = match client.post(&format!("webhooks/{id}/test"), &json!({}), None) {
                Ok(s) => s,
                Err(e) => {
                    step.fail("Not sent", "");
                    return Err(e.into());
                }
            };
            step.done("Test event queued", &str_of(&sent, "event_id"));
            if cli.json {
                print_json(&sent);
            }
        }
        WebhooksCommand::Remove { id, yes } => {
            if !yes && !ui::confirm(&format!("Delete {id}? It stops receiving events."))? {
                ui.warn("Nothing deleted.");
                return Ok(ExitCode::FAILURE);
            }
            let step = ui.step(&format!("Deleting {id}"));
            if let Err(e) = client.delete(&format!("webhooks/{id}")) {
                step.fail("Not deleted", "");
                return Err(e.into());
            }
            step.done(&format!("Deleted {id}"), "");
            if cli.json {
                print_json(&json!({ "deleted": true, "id": id }));
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn trimmed(values: &[String]) -> Vec<String> {
    values.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

fn state(endpoint: &Value) -> String {
    match (
        endpoint.get("enabled").and_then(Value::as_bool),
        endpoint.get("disabled_reason").and_then(Value::as_str),
    ) {
        (Some(true), _) => "enabled",
        (_, Some("failing")) => "failing",
        _ => "disabled",
    }
    .to_string()
}

fn events(endpoint: &Value) -> String {
    let list: Vec<&str> = endpoint
        .get("events")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if list == ["*"] { "all events".to_string() } else { list.join(", ") }
}

fn list(cli: &Cli, ui: Ui, client: &api::Client, limit: u16, cursor: Option<&str>) -> Result<()> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(c) = cursor {
        query.push(("cursor", c.to_string()));
    }

    let step = ui.step("Fetching webhooks");
    let page = client.get("webhooks", &query);
    step.clear();
    let page = page?;

    if cli.json {
        print_json(&page);
        return Ok(());
    }

    let endpoints = page.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    if endpoints.is_empty() {
        println!("No webhooks yet. Add one with: pulsesend webhooks add --url https://…");
        return Ok(());
    }

    let rows: Vec<[String; 4]> = endpoints
        .iter()
        .map(|e| [str_of(e, "id"), state(e), str_of(e, "url"), events(e)])
        .collect();
    print_table(
        ui,
        [
            Col::Dim("ID", 40),
            Col::Status("STATE"),
            Col::Plain("URL", 50),
            Col::Plain("EVENTS", 50),
        ],
        &rows,
    );
    print_next(&page, "webhooks list", limit);

    Ok(())
}

/// Détails de l'endpoint ; le secret n'est présent qu'à la création et à la rotation.
fn show(cli: &Cli, ui: Ui, endpoint: &Value) {
    if cli.json {
        print_json(endpoint);
        return;
    }

    ui.blank();
    let rows = [
        ("id", str_of(endpoint, "id")),
        ("state", ui::status(&state(endpoint))),
        ("url", str_of(endpoint, "url")),
        ("events", events(endpoint)),
    ];
    for (name, value) in rows {
        println!("{}{value}", ui::key(name, 9));
    }

    if let Some(secret) = endpoint.get("secret").and_then(Value::as_str) {
        println!("{}{}", ui::key("secret", 9), ui::strong(secret));
        println!();
        println!(
            "{}",
            ui::label("Store this secret now: it is not shown again. It verifies the X-PulseSend-Signature header.")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_state_and_events() {
        assert_eq!(state(&json!({ "enabled": true })), "enabled");
        assert_eq!(state(&json!({ "enabled": false, "disabled_reason": "failing" })), "failing");
        assert_eq!(state(&json!({ "enabled": false, "disabled_reason": null })), "disabled");
        assert_eq!(events(&json!({ "events": ["*"] })), "all events");
        assert_eq!(
            events(&json!({ "events": ["email.delivered", "email.bounced"] })),
            "email.delivered, email.bounced"
        );
    }
}
