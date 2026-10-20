//! pulsesend : CLI de l'API PulseSend (envoi d'emails transactionnels).
//! Commandes : login, logout, whoami, send, emails list, emails get.
//! Résultats sur stdout ; étapes et spinners sur stderr (voir ui.rs).

mod api;
mod config;
mod ui;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value, json};
use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use ui::Ui;

#[derive(Parser)]
#[command(name = "pulsesend", version, about = "Send transactional email with PulseSend from your terminal.")]
struct Cli {
    /// API key (ps_live_… or ps_test_…). Defaults to the key saved by `pulsesend login`.
    #[arg(long, global = true, env = "PULSESEND_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// API base URL.
    #[arg(long, global = true, env = "PULSESEND_BASE_URL")]
    base_url: Option<String>,

    /// Print raw JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save an API key for the next commands.
    Login,
    /// Remove the saved API key.
    Logout,
    /// Show which key and API the CLI uses, and check that the key works.
    Whoami,
    /// Send an email.
    Send(SendArgs),
    /// List and inspect sent emails.
    #[command(subcommand)]
    Emails(EmailsCommand),
}

#[derive(Args)]
struct SendArgs {
    /// Sender, on a verified domain: "Acme <hello@acme.com>" or hello@acme.com.
    #[arg(long)]
    from: String,
    /// Recipient. Repeat or separate with commas (50 recipients max, cc and bcc included).
    #[arg(long, required = true, value_delimiter = ',')]
    to: Vec<String>,
    #[arg(long)]
    subject: String,
    /// HTML body.
    #[arg(long, conflicts_with = "html_file")]
    html: Option<String>,
    /// Read the HTML body from a file, or from stdin with "-".
    #[arg(long)]
    html_file: Option<String>,
    /// Plain-text body.
    #[arg(long, conflicts_with = "text_file")]
    text: Option<String>,
    /// Read the text body from a file, or from stdin with "-".
    #[arg(long)]
    text_file: Option<String>,
    #[arg(long, value_delimiter = ',')]
    cc: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    bcc: Vec<String>,
    #[arg(long)]
    reply_to: Option<String>,
    /// Tag to find the email later. Repeatable (10 max).
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Custom header, "Name: value". Repeatable (20 max).
    #[arg(long = "header")]
    headers: Vec<String>,
    /// Makes retries safe: the same key within 24 hours returns the first response.
    #[arg(long)]
    idempotency_key: Option<String>,
    /// Wait until the email is delivered (or bounces). Exits with 1 if it is not delivered.
    #[arg(long)]
    wait: bool,
    /// How long --wait waits, in seconds.
    #[arg(long, default_value_t = 60, requires = "wait")]
    wait_timeout: u64,
}

#[derive(Subcommand)]
enum EmailsCommand {
    /// List the latest emails, newest first.
    List {
        /// Emails per page (1 to 100).
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Cursor of the next page, printed at the end of the previous one.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show one email.
    Get {
        /// Email id (em_…).
        id: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui::new(cli.json);
    let base = base_url(&cli);

    match run(&cli, ui, &base) {
        Ok(code) => code,
        Err(e) => {
            ui::print_error(&e, &base);
            ExitCode::FAILURE
        }
    }
}

fn base_url(cli: &Cli) -> String {
    let saved = config::load().unwrap_or_default();

    cli.base_url.clone().or(saved.base_url).unwrap_or_else(|| config::DEFAULT_BASE_URL.to_string())
}

fn run(cli: &Cli, ui: Ui, base: &str) -> Result<ExitCode> {
    let saved = config::load()?;

    match &cli.command {
        Command::Login => return login(cli, ui, base).map(|()| ExitCode::SUCCESS),
        Command::Logout => {
            if config::delete()? {
                ui.ok("Logged out", "the saved API key was removed");
            } else {
                ui.warn("No saved API key.");
            }
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    let key = cli.api_key.clone().or(saved.api_key).context("no API key: run `pulsesend login`, or set PULSESEND_API_KEY")?;
    let client = api::Client::new(base, &key);

    match &cli.command {
        Command::Whoami => whoami(cli, ui, &client, base, &key).map(|()| ExitCode::SUCCESS),
        Command::Send(args) => send(cli, ui, &client, args),
        Command::Emails(EmailsCommand::List { limit, cursor }) => list(cli, ui, &client, *limit, cursor.as_deref()).map(|()| ExitCode::SUCCESS),
        Command::Emails(EmailsCommand::Get { id }) => get(cli, ui, &client, id).map(|()| ExitCode::SUCCESS),
        Command::Login | Command::Logout => unreachable!(),
    }
}

fn login(cli: &Cli, ui: Ui, base: &str) -> Result<()> {
    let key = match &cli.api_key {
        Some(k) => k.clone(),
        None => {
            if !std::io::stdin().is_terminal() {
                bail!("no terminal to type the key in: pass --api-key or set PULSESEND_API_KEY");
            }
            ui.note(&format!("{}  {}", ui::strong("PulseSend"), ui::label(concat!("v", env!("CARGO_PKG_VERSION")))));
            ui.note(&ui::label(&format!("Create a key at {}/api-keys", base.trim_end_matches('/'))));
            ui.note("");
            ui::read_masked("API key")?
        }
    };

    if !key.starts_with("ps_live_") && !key.starts_with("ps_test_") {
        bail!("this does not look like a PulseSend key (ps_live_… or ps_test_…)");
    }

    // Vérifie la clé avant de l'enregistrer : une clé fausse ou révoquée renvoie 401.
    let step = ui.step("Checking the key");
    if let Err(e) = api::Client::new(base, &key).get("emails", &[("limit", "1".into())]) {
        step.fail("Key refused", "");
        return Err(e.into());
    }
    step.done("Key works", &config::mask(&key));

    let step = ui.step("Saving");
    let mut saved = config::load()?;
    saved.api_key = Some(key.clone());
    if cli.base_url.is_some() {
        saved.base_url = cli.base_url.clone();
    }
    let path = config::save(&saved)?;
    step.done("Saved", "");

    if cli.json {
        print_json(&json!({ "key": config::mask(&key), "mode": mode(&key), "config": path.display().to_string() }));
        return Ok(());
    }

    println!();
    println!("{} · {}", ui::accent("Logged in"), ui::mode(mode(&key)));
    println!("{}", ui::label(&format!("Key saved in {} (only you can read it)", path.display())));
    println!();
    println!("{} pulsesend send --help", ui::label("Next:"));

    Ok(())
}

fn whoami(cli: &Cli, ui: Ui, client: &api::Client, base: &str, key: &str) -> Result<()> {
    let source = if std::env::var_os("PULSESEND_API_KEY").is_some() {
        "PULSESEND_API_KEY"
    } else if cli.api_key.is_some() {
        "--api-key"
    } else {
        "saved config"
    };

    let step = ui.step("Checking the key");
    let check = client.get("emails", &[("limit", "1".into())]);
    match &check {
        Ok(_) => step.done("Key works", ""),
        Err(_) => step.fail("Key refused", ""),
    }

    if cli.json {
        print_json(&json!({
            "key": config::mask(key),
            "mode": mode(key),
            "source": source,
            "base_url": base,
            "config": config::path()?.display().to_string(),
            "ok": check.is_ok(),
        }));
    } else {
        println!("{:<8}{} · {} · from {source}", ui::label("Key"), config::mask(key), ui::mode(mode(key)));
        println!("{:<8}{base}", ui::label("API"));
        println!("{:<8}{}", ui::label("Config"), config::path()?.display());
    }

    check?;

    Ok(())
}

fn send(cli: &Cli, ui: Ui, client: &api::Client, args: &SendArgs) -> Result<ExitCode> {
    let html = body(args.html.as_deref(), args.html_file.as_deref())?;
    let text = body(args.text.as_deref(), args.text_file.as_deref())?;

    if html.is_none() && text.is_none() {
        bail!("provide a body: --html, --html-file, --text or --text-file");
    }

    let mut payload = Map::new();
    payload.insert("from".into(), json!(args.from));
    payload.insert("to".into(), json!(trimmed(&args.to)));
    payload.insert("subject".into(), json!(args.subject));
    if let Some(h) = html {
        payload.insert("html".into(), json!(h));
    }
    if let Some(t) = text {
        payload.insert("text".into(), json!(t));
    }
    if !args.cc.is_empty() {
        payload.insert("cc".into(), json!(trimmed(&args.cc)));
    }
    if !args.bcc.is_empty() {
        payload.insert("bcc".into(), json!(trimmed(&args.bcc)));
    }
    if let Some(r) = &args.reply_to {
        payload.insert("reply_to".into(), json!(r));
    }
    if !args.tags.is_empty() {
        payload.insert("tags".into(), json!(args.tags));
    }
    if !args.headers.is_empty() {
        payload.insert("headers".into(), Value::Object(parse_headers(&args.headers)?));
    }
    ui.ok("Options checked", "");

    let step = ui.step("Sending to the API");
    let mut email = match client.post("emails", &Value::Object(payload), args.idempotency_key.as_deref()) {
        Ok(email) => email,
        Err(e) => {
            step.fail("Not sent", "");
            return Err(e.into());
        }
    };
    let ms = step.elapsed().as_millis();
    step.done("Queued", &format!("{} · {ms} ms", str_of(&email, "id")));

    let mut code = ExitCode::SUCCESS;
    if args.wait {
        let (final_email, delivered) = wait_for_delivery(ui, client, email, Duration::from_secs(args.wait_timeout))?;
        email = final_email;
        if !delivered {
            code = ExitCode::FAILURE;
        }
    }

    if cli.json {
        print_json(&email);
    } else {
        ui.blank();
        print_summary(&email);
    }

    Ok(code)
}

/// Relit l'email chaque seconde jusqu'à une issue : délivré, rebond, plainte, échec.
/// Avec une clé de test, l'email n'est jamais délivré : « sent » suffit.
fn wait_for_delivery(ui: Ui, client: &api::Client, mut email: Value, timeout: Duration) -> Result<(Value, bool)> {
    let id = str_of(&email, "id");
    let test = email.get("test_mode").and_then(Value::as_bool) == Some(true);
    let to = email.get("to").and_then(Value::as_array).map(|a| join(a)).unwrap_or_default();
    let step = ui.step("Waiting for delivery");
    let start = Instant::now();

    loop {
        let status = str_of(&email, "status");
        let secs = format!("{:.1}s", start.elapsed().as_secs_f64());

        match status.as_str() {
            "delivered" => {
                step.done(&format!("Delivered to {to}"), &format!("in {secs}"));
                return Ok((email, true));
            }
            "sent" if test => {
                step.done("Accepted", "test key: recorded, never delivered");
                return Ok((email, true));
            }
            "bounced" | "complained" | "failed" | "canceled" => {
                step.fail(&format!("{} for {to}", capitalize(&status)), &format!("after {secs}"));
                return Ok((email, false));
            }
            _ => {}
        }

        if start.elapsed() >= timeout {
            step.warn(&format!("Still {status} after {secs}: check later with pulsesend emails get {id}"));
            return Ok((email, false));
        }

        step.detail(&format!("{status} · {secs}"));
        std::thread::sleep(Duration::from_millis(1000));
        email = client.get(&format!("emails/{id}"), &[])?;
    }
}

fn print_summary(email: &Value) {
    let to = email.get("to").and_then(Value::as_array).map(|a| join(a)).unwrap_or_default();
    let credits = email.get("credits_charged").map(Value::to_string).unwrap_or_default();
    let rows = [
        ("id", str_of(email, "id")),
        ("to", to),
        ("status", ui::status(&str_of(email, "status"))),
        ("credits", credits),
    ];

    for (name, value) in rows {
        println!("  {:<9}{value}", ui::label(name));
    }
    if email.get("test_mode").and_then(Value::as_bool) == Some(true) {
        println!("  {:<9}{}", ui::label("mode"), ui::mode("test"));
    }
}

fn list(cli: &Cli, ui: Ui, client: &api::Client, limit: u16, cursor: Option<&str>) -> Result<()> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(c) = cursor {
        query.push(("cursor", c.to_string()));
    }

    let step = ui.step("Fetching emails");
    let page = client.get("emails", &query);
    step.clear();
    let page = page?;

    if cli.json {
        print_json(&page);
        return Ok(());
    }

    let emails = page.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    if emails.is_empty() {
        println!("No emails yet.");
        return Ok(());
    }

    let rows: Vec<[String; 5]> = emails
        .iter()
        .map(|e| {
            [
                str_of(e, "id"),
                str_of(e, "status"),
                e.get("to").and_then(Value::as_array).map(|a| join(a)).unwrap_or_default(),
                str_of(e, "subject"),
                str_of(e, "created_at"),
            ]
        })
        .collect();
    print_table(ui, &rows);

    if page.get("has_more").and_then(Value::as_bool) == Some(true) {
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            println!("\n{} pulsesend emails list --limit {limit} --cursor {next}", ui::label("More:"));
        }
    }

    Ok(())
}

fn get(cli: &Cli, ui: Ui, client: &api::Client, id: &str) -> Result<()> {
    let step = ui.step(&format!("Fetching {id}"));
    let email = client.get(&format!("emails/{id}"), &[]);
    step.clear();
    let email = email?;

    if cli.json {
        print_json(&email);
        return Ok(());
    }

    let list = |k: &str| email.get(k).and_then(Value::as_array).map(|a| join(a)).filter(|s| !s.is_empty());
    let mut lines = vec![
        ("id", str_of(&email, "id")),
        ("status", ui::status(&str_of(&email, "status"))),
        ("from", str_of(&email, "from")),
        ("to", list("to").unwrap_or_default()),
    ];
    if let Some(cc) = list("cc") {
        lines.push(("cc", cc));
    }
    if let Some(bcc) = list("bcc") {
        lines.push(("bcc", bcc));
    }
    lines.push(("subject", str_of(&email, "subject")));
    if let Some(tags) = list("tags") {
        lines.push(("tags", tags));
    }
    let test = email.get("test_mode").and_then(Value::as_bool) == Some(true);
    lines.push(("mode", ui::mode(if test { "test" } else { "live" })));
    lines.push(("credits", email.get("credits_charged").map(Value::to_string).unwrap_or_default()));
    lines.push(("created", str_of(&email, "created_at")));

    for (name, value) in lines {
        println!("{:<10}{value}", ui::label(name));
    }

    Ok(())
}

/// Corps passé en clair, ou lu dans un fichier (« - » = entrée standard).
fn body(inline: Option<&str>, file: Option<&str>) -> Result<Option<String>> {
    match (inline, file) {
        (Some(s), _) => Ok(Some(s.to_string())),
        (None, Some("-")) => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
            Ok(Some(s))
        }
        (None, Some(path)) => Ok(Some(std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?)),
        (None, None) => Ok(None),
    }
}

fn parse_headers(raw: &[String]) -> Result<Map<String, Value>> {
    let mut headers = Map::new();
    for h in raw {
        let (name, value) = h.split_once(':').with_context(|| format!("header \"{h}\" must look like \"Name: value\""))?;
        headers.insert(name.trim().to_string(), json!(value.trim()));
    }

    Ok(headers)
}

fn trimmed(values: &[String]) -> Vec<String> {
    values.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

fn mode(key: &str) -> &'static str {
    if key.starts_with("ps_test_") { "test" } else { "live" }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn join(values: &[Value]) -> String {
    values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

/// Tableau des emails : en-tête grisé, statut en couleur ; les lignes arrivent une à une
/// dans un terminal.
fn print_table(ui: Ui, rows: &[[String; 5]]) {
    const HEADERS: [&str; 5] = ["ID", "STATUS", "TO", "SUBJECT", "SENT"];
    // Les colonnes longues (destinataires, sujet) sont coupées pour garder une ligne par email.
    const MAX: [usize; 5] = [40, 18, 32, 40, 20];
    let cell = |s: &str, i: usize| -> String {
        if s.chars().count() > MAX[i] { s.chars().take(MAX[i] - 1).collect::<String>() + "…" } else { s.to_string() }
    };
    // La colonne STATUS affiche « ● statut » : deux caractères de plus.
    let width = |s: &str, i: usize| cell(s, i).chars().count() + if i == 1 { 2 } else { 0 };
    let mut widths = HEADERS.map(|h| h.chars().count());
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(c, i));
        }
    }
    let pad = |text: String, visible: usize, w: usize| format!("{text}{}", " ".repeat(w.saturating_sub(visible)));

    let header: Vec<String> = HEADERS.iter().enumerate().map(|(i, h)| pad(ui::label(h), h.chars().count(), widths[i])).collect();
    println!("{}", header.join("  ").trim_end());

    let animate = ui.animate() && std::io::stdout().is_terminal();
    for row in rows {
        let cols: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let visible = width(c, i);
                let text = match i {
                    0 => ui::label(&cell(c, i)),
                    1 => ui::status(c),
                    3 => ui::label(&cell(c, i)),
                    _ => cell(c, i),
                };
                pad(text, visible, widths[i])
            })
            .collect();
        println!("{}", cols.join("  ").trim_end());
        if animate {
            std::thread::sleep(Duration::from_millis(18));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers() {
        let h = parse_headers(&["X-Entity-Ref-ID: order-8812".into()]).unwrap();
        assert_eq!(h["X-Entity-Ref-ID"], json!("order-8812"));
        assert!(parse_headers(&["missing colon".into()]).is_err());
    }

    #[test]
    fn trims_recipients() {
        assert_eq!(trimmed(&[" a@x.com".into(), "".into(), "b@x.com ".into()]), vec!["a@x.com", "b@x.com"]);
    }

    #[test]
    fn detects_mode() {
        assert_eq!(mode("ps_test_abc"), "test");
        assert_eq!(mode("ps_live_abc"), "live");
    }

    #[test]
    fn capitalizes() {
        assert_eq!(capitalize("bounced"), "Bounced");
        assert_eq!(capitalize(""), "");
    }
}
