//! pulsesend domains : liste, ajout, enregistrements DNS, vérification, suppression.
//! Clé avec le scope domains:read (lecture) ou domains:write (le reste).

use crate::ui::{self, Ui};
use crate::{Cli, Col, api, print_json, print_next, print_table, str_of};
use anyhow::Result;
use clap::Subcommand;
use serde_json::{Value, json};
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Subcommand)]
pub enum DomainsCommand {
    /// List your sending domains.
    List {
        /// Domains per page (1 to 100).
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Cursor of the next page, printed at the end of the previous one.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Add a domain and print the DNS records to publish.
    Add {
        /// The domain you send from, e.g. acme.com or mail.acme.com.
        name: String,
    },
    /// Show a domain and the state of its DNS records.
    Get {
        /// Domain name or id (dom_…).
        domain: String,
    },
    /// Turn open and click tracking on or off for the emails sent from a domain.
    Update {
        /// Domain name or id (dom_…).
        domain: String,
        /// Count opens with a tracking pixel: on or off.
        #[arg(long, value_name = "on|off", value_parser = crate::parse_switch)]
        track_opens: Option<bool>,
        /// Count clicks by rewriting links: on or off.
        #[arg(long, value_name = "on|off", value_parser = crate::parse_switch)]
        track_clicks: Option<bool>,
        /// Your own subdomain for tracked links, e.g. links.acme.com, or `off` to remove it.
        #[arg(long, value_name = "host|off")]
        tracking_domain: Option<String>,
    },
    /// Check the DNS records now.
    Verify {
        /// Domain name or id (dom_…).
        domain: String,
        /// Keep checking until the domain is verified. Exits with 1 if it is not.
        #[arg(long)]
        wait: bool,
        /// How long --wait waits, in seconds.
        #[arg(long, default_value_t = 600, requires = "wait")]
        wait_timeout: u64,
    },
    /// Remove a domain: emails from it are refused afterwards.
    Remove {
        /// Domain name or id (dom_…).
        domain: String,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
}

pub fn run(cli: &Cli, ui: Ui, client: &api::Client, command: &DomainsCommand) -> Result<ExitCode> {
    match command {
        DomainsCommand::List { limit, cursor } => list(cli, ui, client, *limit, cursor.as_deref()),
        DomainsCommand::Add { name } => {
            let step = ui.step(&format!("Adding {name}"));
            let domain = client.post("domains", &json!({ "name": name }), None);
            let domain = match domain {
                Ok(d) => d,
                Err(e) => {
                    step.fail("Not added", "");
                    return Err(e.into());
                }
            };
            step.done(&format!("Added {}", str_of(&domain, "name")), &str_of(&domain, "id"));
            show(cli, ui, &domain, true);
            Ok(ExitCode::SUCCESS)
        }
        DomainsCommand::Get { domain } => {
            let step = ui.step(&format!("Fetching {domain}"));
            let found = client.get(&path(domain), &[]);
            step.clear();
            show(cli, ui, &found?, false);
            Ok(ExitCode::SUCCESS)
        }
        DomainsCommand::Update {
            domain,
            track_opens,
            track_clicks,
            tracking_domain,
        } => {
            let body = update_body(*track_opens, *track_clicks, tracking_domain.as_deref());
            if body.as_object().is_some_and(serde_json::Map::is_empty) {
                anyhow::bail!("Nothing to update: pass --track-opens, --track-clicks or --tracking-domain.");
            }
            let step = ui.step(&format!("Updating {domain}"));
            let updated = match client.patch(&path(domain), &body) {
                Ok(d) => d,
                Err(e) => {
                    step.fail("Not updated", "");
                    return Err(e.into());
                }
            };
            step.done(&format!("Updated {}", str_of(&updated, "name")), &tracking_line(&updated));
            if cli.json {
                print_json(&updated);
            } else if let Some(line) = tracking_record_line(&updated) {
                ui.blank();
                println!("Add this record at your DNS provider: {line}");
            }
            Ok(ExitCode::SUCCESS)
        }
        DomainsCommand::Verify {
            domain,
            wait,
            wait_timeout,
        } => verify(cli, ui, client, domain, *wait, Duration::from_secs(*wait_timeout)),
        DomainsCommand::Remove { domain, yes } => {
            if !yes && !ui::confirm(&format!("Remove {domain}? Emails from it will be refused."))? {
                ui.warn("Nothing removed.");
                return Ok(ExitCode::FAILURE);
            }
            let step = ui.step(&format!("Removing {domain}"));
            match client.delete(&path(domain)) {
                Ok(_) => step.done(&format!("Removed {domain}"), ""),
                Err(e) => {
                    step.fail("Not removed", "");
                    return Err(e.into());
                }
            }
            if cli.json {
                print_json(&json!({ "deleted": true, "domain": domain }));
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Un nom ou un id : l'API accepte les deux.
fn path(domain: &str) -> String {
    format!("domains/{}", domain.trim())
}

fn list(cli: &Cli, ui: Ui, client: &api::Client, limit: u16, cursor: Option<&str>) -> Result<ExitCode> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(c) = cursor {
        query.push(("cursor", c.to_string()));
    }

    let step = ui.step("Fetching domains");
    let page = client.get("domains", &query);
    step.clear();
    let page = page?;

    if cli.json {
        print_json(&page);
        return Ok(ExitCode::SUCCESS);
    }

    let domains = page.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    if domains.is_empty() {
        println!("No domains yet. Add one with: pulsesend domains add acme.com");
        return Ok(ExitCode::SUCCESS);
    }

    let rows: Vec<[String; 5]> = domains
        .iter()
        .map(|d| {
            [
                str_of(d, "name"),
                str_of(d, "status"),
                yes_no(d, "spf"),
                yes_no(d, "dmarc"),
                str_of(d, "id"),
            ]
        })
        .collect();
    print_table(
        ui,
        [
            Col::Plain("DOMAIN", 40),
            Col::Status("STATUS"),
            Col::Plain("SPF", 3),
            Col::Plain("DMARC", 5),
            Col::Dim("ID", 40),
        ],
        &rows,
    );
    print_next(&page, "domains list", limit);

    Ok(ExitCode::SUCCESS)
}

/// Relance la vérification, ou la répète toutes les 15 s avec --wait (l'API en accepte 6 par minute).
fn verify(cli: &Cli, ui: Ui, client: &api::Client, domain: &str, wait: bool, timeout: Duration) -> Result<ExitCode> {
    let step = ui.step(&format!("Checking the DNS of {domain}"));
    let start = Instant::now();

    let result = loop {
        let checked = match client.post(&format!("{}/verify", path(domain)), &json!({}), None) {
            Ok(d) => d,
            Err(e) => {
                step.fail("Check failed", "");
                return Err(e.into());
            }
        };
        let status = str_of(&checked, "status");
        let found = records(&checked).iter().filter(|r| str_of(r, "status") == "found").count();
        let total = records(&checked).len();

        if status == "verified" {
            step.done(&format!("{domain} is verified"), &format!("{found}/{total} records found"));
            break (checked, true);
        }
        if !wait || start.elapsed() >= timeout {
            step.warn(&format!("{domain} is {status}: {found}/{total} records found"));
            break (checked, false);
        }

        step.detail(&format!("{found}/{total} records · {:.0}s", start.elapsed().as_secs_f64()));
        std::thread::sleep(Duration::from_secs(15));
    };

    let (checked, verified) = result;
    show(cli, ui, &checked, !verified);

    Ok(if verified { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Seuls les réglages passés partent : les autres restent tels quels. `off` retire le domaine de suivi.
fn update_body(opens: Option<bool>, clicks: Option<bool>, tracking_domain: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    if let Some(on) = opens {
        body.insert("track_opens".into(), json!(on));
    }
    if let Some(on) = clicks {
        body.insert("track_clicks".into(), json!(on));
    }
    if let Some(host) = tracking_domain.map(str::trim) {
        let off = host.is_empty() || ["off", "none"].contains(&host.to_ascii_lowercase().as_str());
        body.insert("tracking_domain".into(), if off { Value::Null } else { json!(host) });
    }
    Value::Object(body)
}

/// « links.acme.com (pending) », ou vide sans domaine de suivi.
fn tracking_domain_line(domain: &Value) -> String {
    let tracking = domain.get("tracking").cloned().unwrap_or_default();
    match tracking.get("domain").and_then(Value::as_str) {
        Some(host) => {
            let status = tracking.get("domain_status").and_then(Value::as_str).unwrap_or("pending");
            format!("{host} ({status})")
        }
        None => String::new(),
    }
}

/// Le CNAME à publier tant que le domaine de suivi n'est pas actif : « CNAME links → track.… ».
fn tracking_record_line(domain: &Value) -> Option<String> {
    let tracking = domain.get("tracking")?;
    if tracking.get("domain_status").and_then(Value::as_str) == Some("active") {
        return None;
    }
    let record = tracking.get("domain_record").filter(|r| r.is_object())?;
    Some(format!("CNAME {} → {}", str_of(record, "host"), str_of(record, "value")))
}

/// « opens on · clicks off ».
fn tracking_line(domain: &Value) -> String {
    let tracking = domain.get("tracking").cloned().unwrap_or_default();
    let state = |k: &str| {
        if tracking.get(k).and_then(Value::as_bool) == Some(true) {
            "on"
        } else {
            "off"
        }
    };
    format!("opens {} · clicks {}", state("opens"), state("clicks"))
}

fn records(domain: &Value) -> Vec<Value> {
    domain.get("records").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn yes_no(v: &Value, key: &str) -> String {
    if v.get(key).and_then(Value::as_bool) == Some(true) {
        "yes"
    } else {
        "no"
    }
    .to_string()
}

/// Détails du domaine, puis ses enregistrements ; `hint` ajoute la marche à suivre.
fn show(cli: &Cli, ui: Ui, domain: &Value, hint: bool) {
    if cli.json {
        print_json(domain);
        return;
    }

    ui.blank();
    let mut lines = vec![
        ("domain", str_of(domain, "name")),
        ("status", ui::status(&str_of(domain, "status"))),
        ("id", str_of(domain, "id")),
    ];
    let provider = str_of(domain, "dns_provider");
    if !provider.is_empty() {
        lines.push(("dns", provider));
    }
    lines.push(("tracking", tracking_line(domain)));
    let host = tracking_domain_line(domain);
    if !host.is_empty() {
        lines.push(("links", host));
    }
    for (name, value) in lines {
        println!("{}{value}", ui::key(name, 9));
    }
    if let Some(line) = tracking_record_line(domain) {
        println!("{}{line}", ui::key("", 9));
    }
    println!();

    let rows: Vec<[String; 5]> = records(domain)
        .iter()
        .map(|r| {
            let value = match r.get("priority").and_then(Value::as_u64) {
                Some(p) => format!("{p} {}", str_of(r, "value")),
                None => str_of(r, "value"),
            };
            [
                str_of(r, "type"),
                str_of(r, "host"),
                value,
                str_of(r, "status"),
                if r.get("required").and_then(Value::as_bool) == Some(true) {
                    "required"
                } else {
                    "recommended"
                }
                .to_string(),
            ]
        })
        .collect();
    print_table(
        ui,
        [
            Col::Plain("TYPE", 5),
            Col::Plain("NAME", 60),
            Col::Plain("VALUE", 70),
            Col::Status("CHECK"),
            Col::Dim("", 11),
        ],
        &rows,
    );

    if hint {
        println!();
        println!(
            "{} publish these records at your DNS provider, then run: pulsesend domains verify {} --wait",
            ui::label("Next:"),
            str_of(domain, "name")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_paths_and_flags() {
        assert_eq!(path(" acme.com "), "domains/acme.com");
        assert_eq!(yes_no(&json!({ "spf": true }), "spf"), "yes");
        assert_eq!(yes_no(&json!({}), "dmarc"), "no");
    }

    #[test]
    fn builds_tracking_updates() {
        assert_eq!(update_body(Some(true), None, None), json!({ "track_opens": true }));
        assert_eq!(update_body(None, Some(false), None), json!({ "track_clicks": false }));
        assert_eq!(update_body(None, None, None), json!({}));
        assert_eq!(
            update_body(None, None, Some("links.acme.com")),
            json!({ "tracking_domain": "links.acme.com" })
        );
        assert_eq!(update_body(None, None, Some("off")), json!({ "tracking_domain": null }));
        let pending = json!({ "tracking": {
            "domain": "links.acme.com",
            "domain_status": "pending",
            "domain_record": { "host": "links", "value": "track.example.com" }
        } });
        assert_eq!(tracking_domain_line(&pending), "links.acme.com (pending)");
        assert_eq!(tracking_record_line(&pending).as_deref(), Some("CNAME links → track.example.com"));
        assert_eq!(tracking_record_line(&json!({ "tracking": { "domain_status": "active" } })), None);
        assert_eq!(tracking_domain_line(&json!({})), "");
        assert_eq!(
            tracking_line(&json!({ "tracking": { "opens": true, "clicks": false } })),
            "opens on · clicks off"
        );
        assert_eq!(tracking_line(&json!({})), "opens off · clicks off");
    }
}
