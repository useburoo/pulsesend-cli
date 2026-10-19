//! pulsesend : CLI de l'API PulseSend (envoi d'emails transactionnels).
//! Commandes : login, logout, whoami, send, emails list, emails get.

mod api;
mod config;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value, json};
use std::io::{IsTerminal, Read};
use std::process::ExitCode;

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

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let saved = config::load()?;
    let base = cli.base_url.clone().or(saved.base_url.clone()).unwrap_or_else(|| config::DEFAULT_BASE_URL.to_string());

    match &cli.command {
        Command::Login => return login(&cli, &base),
        Command::Logout => {
            if config::delete()? {
                println!("Logged out: the saved API key was removed.");
            } else {
                println!("No saved API key.");
            }
            return Ok(());
        }
        _ => {}
    }

    let key = cli.api_key.clone().or(saved.api_key.clone()).context("no API key: run `pulsesend login`, or set PULSESEND_API_KEY")?;
    let client = api::Client::new(&base, &key);

    match &cli.command {
        Command::Whoami => whoami(&cli, &client, &base, &key),
        Command::Send(args) => send(&cli, &client, args),
        Command::Emails(EmailsCommand::List { limit, cursor }) => list(&cli, &client, *limit, cursor.as_deref()),
        Command::Emails(EmailsCommand::Get { id }) => get(&cli, &client, id),
        Command::Login | Command::Logout => unreachable!(),
    }
}

fn login(cli: &Cli, base: &str) -> Result<()> {
    let key = match &cli.api_key {
        Some(k) => k.clone(),
        None => {
            if !std::io::stdin().is_terminal() {
                bail!("no terminal to type the key in: pass --api-key or set PULSESEND_API_KEY");
            }
            println!("Create a key in the dashboard: {}/api-keys", base.trim_end_matches('/'));
            rpassword::prompt_password("API key: ")?.trim().to_string()
        }
    };

    if !key.starts_with("ps_live_") && !key.starts_with("ps_test_") {
        bail!("this does not look like a PulseSend key (ps_live_… or ps_test_…)");
    }

    // Vérifie la clé avant de l'enregistrer : une clé fausse ou révoquée renvoie 401.
    api::Client::new(base, &key).get("emails", &[("limit", "1".into())])?;

    let mut saved = config::load()?;
    saved.api_key = Some(key.clone());
    if cli.base_url.is_some() {
        saved.base_url = cli.base_url.clone();
    }
    let path = config::save(&saved)?;

    println!("Logged in with {} ({} mode).", config::mask(&key), mode(&key));
    println!("Key saved in {}", path.display());

    Ok(())
}

fn whoami(cli: &Cli, client: &api::Client, base: &str, key: &str) -> Result<()> {
    let source = if std::env::var_os("PULSESEND_API_KEY").is_some() {
        "PULSESEND_API_KEY"
    } else if cli.api_key.is_some() {
        "--api-key"
    } else {
        "saved config"
    };
    let check = client.get("emails", &[("limit", "1".into())]);

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
        println!("Key:     {} ({} mode, from {source})", config::mask(key), mode(key));
        println!("API:     {base}");
        println!("Config:  {}", config::path()?.display());
    }

    check?;
    if !cli.json {
        println!("Status:  key works");
    }

    Ok(())
}

fn send(cli: &Cli, client: &api::Client, args: &SendArgs) -> Result<()> {
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

    let email = client.post("emails", &Value::Object(payload), args.idempotency_key.as_deref())?;

    if cli.json {
        print_json(&email);
    } else {
        println!("Queued {} ({})", str_of(&email, "id"), str_of(&email, "status"));
        if email.get("test_mode").and_then(Value::as_bool) == Some(true) {
            println!("Test key: the email is recorded but never delivered.");
        }
    }

    Ok(())
}

fn list(cli: &Cli, client: &api::Client, limit: u16, cursor: Option<&str>) -> Result<()> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(c) = cursor {
        query.push(("cursor", c.to_string()));
    }
    let page = client.get("emails", &query)?;

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
    print_table(&["ID", "STATUS", "TO", "SUBJECT", "CREATED"], &rows);

    if page.get("has_more").and_then(Value::as_bool) == Some(true) {
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            println!("\nMore: pulsesend emails list --limit {limit} --cursor {next}");
        }
    }

    Ok(())
}

fn get(cli: &Cli, client: &api::Client, id: &str) -> Result<()> {
    let email = client.get(&format!("emails/{id}"), &[])?;

    if cli.json {
        print_json(&email);
        return Ok(());
    }

    let list = |k: &str| email.get(k).and_then(Value::as_array).map(|a| join(a)).filter(|s| !s.is_empty());
    let mut lines = vec![
        ("ID", str_of(&email, "id")),
        ("Status", str_of(&email, "status")),
        ("From", str_of(&email, "from")),
        ("To", list("to").unwrap_or_default()),
    ];
    if let Some(cc) = list("cc") {
        lines.push(("Cc", cc));
    }
    if let Some(bcc) = list("bcc") {
        lines.push(("Bcc", bcc));
    }
    lines.push(("Subject", str_of(&email, "subject")));
    if let Some(tags) = list("tags") {
        lines.push(("Tags", tags));
    }
    lines.push(("Mode", if email.get("test_mode").and_then(Value::as_bool) == Some(true) { "test".into() } else { "live".into() }));
    lines.push(("Credits", email.get("credits_charged").map(Value::to_string).unwrap_or_default()));
    lines.push(("Created", str_of(&email, "created_at")));

    for (label, value) in lines {
        println!("{:<9}{value}", format!("{label}:"));
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

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn join(values: &[Value]) -> String {
    values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn print_table(headers: &[&str; 5], rows: &[[String; 5]]) {
    // Les colonnes longues (destinataires, sujet) sont coupées pour garder une ligne par email.
    const MAX: [usize; 5] = [40, 16, 32, 40, 20];
    let cell = |s: &str, i: usize| -> String {
        if s.chars().count() > MAX[i] { s.chars().take(MAX[i] - 1).collect::<String>() + "…" } else { s.to_string() }
    };
    let mut widths = headers.map(|h| h.chars().count());
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell(c, i).chars().count());
        }
    }
    let line = |cols: Vec<String>| {
        let parts: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c:<w$}", w = widths[i])).collect();
        println!("{}", parts.join("  ").trim_end());
    };
    line(headers.iter().map(|h| h.to_string()).collect());
    for row in rows {
        line(row.iter().enumerate().map(|(i, c)| cell(c, i)).collect());
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
}
