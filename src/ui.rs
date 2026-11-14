//! Affichage du CLI : étapes avec spinner (stderr), statuts en couleur, encadré d'erreur.
//! Animations seulement dans un vrai terminal ; en pipe ou en CI, des lignes simples ;
//! avec --json, rien sur stderr. NO_COLOR et CLICOLOR sont respectés par `console`.

use crate::api;
use console::{Key, Term, style};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::time::{Duration, Instant};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "⠏"];

#[derive(Clone, Copy)]
pub struct Ui {
    /// Spinners animés (stderr est un terminal, pas de --json).
    animate: bool,
    /// Aucune ligne de progression (--json).
    quiet: bool,
}

impl Ui {
    pub fn new(json: bool) -> Self {
        Self {
            animate: !json && Term::stderr().is_term(),
            quiet: json,
        }
    }

    /// Lance une étape : spinner animé, ou rien tant qu'elle n'est pas finie.
    pub fn step(&self, label: &str) -> Step {
        let bar = self.animate.then(|| {
            let bar = ProgressBar::new_spinner();
            bar.set_style(
                ProgressStyle::with_template("{spinner:.blue} {msg}")
                    .expect("modèle valide")
                    .tick_strings(FRAMES),
            );
            bar.set_message(format!("{label}…"));
            bar.enable_steady_tick(Duration::from_millis(80));
            bar
        });

        Step {
            ui: *self,
            bar,
            label: label.to_string(),
            started: Instant::now(),
        }
    }

    /// Une étape déjà faite, sans attente.
    pub fn ok(&self, label: &str, detail: &str) {
        if !self.quiet {
            eprintln!("{} {label}{}", style("✓").green().bold(), dim_detail(detail));
        }
    }

    pub fn warn(&self, message: &str) {
        if !self.quiet {
            eprintln!("{} {message}", style("!").yellow().bold());
        }
    }

    pub fn blank(&self) {
        if !self.quiet {
            eprintln!();
        }
    }

    /// Lignes de présentation (login) : seulement dans un terminal.
    pub fn note(&self, line: &str) {
        if self.animate {
            eprintln!("{line}");
        }
    }

    pub fn animate(&self) -> bool {
        self.animate
    }
}

pub struct Step {
    ui: Ui,
    bar: Option<ProgressBar>,
    label: String,
    started: Instant,
}

impl Step {
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Texte grisé après le libellé, mis à jour pendant l'attente (« 1.2s »).
    pub fn detail(&self, detail: &str) {
        if let Some(bar) = &self.bar {
            bar.set_message(format!("{}…{}", self.label, dim_detail(detail)));
        }
    }

    pub fn done(self, label: &str, detail: &str) {
        self.clear();
        self.ui.ok(label, detail);
    }

    pub fn fail(self, label: &str, detail: &str) {
        self.clear();
        if !self.ui.quiet {
            eprintln!("{} {label}{}", style("✗").red().bold(), dim_detail(detail));
        }
    }

    pub fn warn(self, label: &str) {
        self.clear();
        self.ui.warn(label);
    }

    /// Efface le spinner sans rien écrire (la suite affiche le résultat).
    pub fn clear(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Step {
    fn drop(&mut self) {
        self.clear();
    }
}

fn dim_detail(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!("  {}", style(detail).dim())
    }
}

/// « ● delivered » dans la couleur du statut.
pub fn status(status: &str) -> String {
    let dot = format!("● {status}");
    match status {
        "delivered" | "verified" | "found" | "enabled" => style(dot).green().bold().to_string(),
        "missing" | "disabled" => style(dot).yellow().to_string(),
        "bounced" | "failed" | "failing" => style(dot).red().bold().to_string(),
        "complained" => style(dot).yellow().bold().to_string(),
        "delivery_delayed" => style(dot).blue().to_string(),
        "canceled" => style(dot).dim().to_string(),
        _ => style(dot).white().to_string(),
    }
}

/// Étiquette grisée alignée sur `width` colonnes. L'alignement se fait AVANT la couleur :
/// les codes ANSI comptent dans `{:<n}`, qui n'alignerait plus rien en terminal.
pub fn key(text: &str, width: usize) -> String {
    style(format!("{text:<width$}")).dim().to_string()
}

pub fn label(text: &str) -> String {
    style(text).dim().to_string()
}

pub fn strong(text: &str) -> String {
    style(text).bold().to_string()
}

pub fn accent(text: &str) -> String {
    style(text).green().bold().to_string()
}

pub fn mode(mode: &str) -> String {
    if mode == "test" {
        style("test mode").yellow().to_string()
    } else {
        style("live mode").green().to_string()
    }
}

/// Saisie masquée : chaque caractère s'affiche en « • », Retour arrière efface.
pub fn read_masked(prompt: &str) -> anyhow::Result<String> {
    let term = Term::stderr();
    term.write_str(&format!("{} {prompt} ", style("?").blue().bold()))?;
    let mut value = String::new();

    loop {
        match term.read_key()? {
            Key::Enter => break,
            Key::Backspace => {
                if value.pop().is_some() {
                    term.clear_chars(1)?;
                }
            }
            Key::CtrlC | Key::Escape => {
                term.write_line("")?;
                anyhow::bail!("canceled");
            }
            Key::Char(c) if !c.is_control() => {
                value.push(c);
                term.write_str("•")?;
            }
            _ => {}
        }
    }

    term.write_line("")?;

    Ok(value.trim().to_string())
}

/// Erreur finale de la commande. Erreur de l'API : titre rouge, code, champ, requête, doc et piste.
pub fn print_error(err: &anyhow::Error, base: &str) {
    let Some(api::Error::Api { status, body, request_id }) = err.downcast_ref::<api::Error>() else {
        let text = format!("{err:#}");
        if text.starts_with("no API key") {
            eprintln!(
                "{} {}",
                style("!").yellow().bold(),
                text.replacen("no API key", "No API key yet", 1)
            );
        } else {
            eprintln!("{} {text}", style("✗").red().bold());
        }
        return;
    };

    let title = body.message.as_deref().unwrap_or("API error");
    eprintln!("{} {}  {}", style("✗").red().bold(), style(title).red().bold(), style(status).dim());

    let rows = [
        ("code", body.code.as_deref()),
        ("field", body.param.as_deref()),
        ("request", request_id.as_deref()),
    ];
    for (name, value) in rows {
        if let Some(value) = value {
            eprintln!("  {}{value}", key(name, 9));
        }
    }

    if let Some(hint) = hint(body.code.as_deref(), base) {
        eprintln!("  {} {hint}", style("→").dim());
    }
    if let Some(url) = &body.doc_url {
        eprintln!("  {} Docs: {url}", style("→").dim());
    }
}

/// Que faire ensuite, pour les erreurs les plus courantes.
fn hint(code: Option<&str>, base: &str) -> Option<String> {
    let base = base.trim_end_matches('/');

    Some(match code? {
        "invalid_api_key" | "missing_api_key" => "Log in again with a valid key: pulsesend login".to_string(),
        "domain_not_verified" => format!("Add and verify the domain: {base}/domains"),
        "insufficient_credits" | "daily_limit_reached" => format!("Buy credits or subscribe to Pro: {base}/billing"),
        "team_paused" => "Sending is paused for this team: check your bounces in the dashboard".to_string(),
        "rate_limited" => "Too many requests: wait a second and retry".to_string(),
        _ => return None,
    })
}

/// Demande « oui / non » sur le terminal (non par défaut). Hors terminal, refuse : passer --yes.
pub fn confirm(question: &str) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("no terminal to confirm in: pass --yes");
    }
    let term = Term::stderr();
    term.write_str(&format!("{} {question} {} ", style("?").blue().bold(), style("[y/N]").dim()))?;
    let answer = term.read_line()?;

    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "o" | "oui"))
}
