//! Client de l'API publique PulseSend (/api/v1). Les erreurs de l'API arrivent au format
//! { "error": { type, code, message, param, doc_url } } et sont rendues telles quelles.

use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub struct Client {
    agent: ureq::Agent,
    base: String,
    key: String,
}

#[derive(Debug, Deserialize)]
pub struct ApiErrorBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub param: Option<String>,
    pub doc_url: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    /// Réponse d'erreur de l'API (4xx / 5xx).
    // Encadré : garde `Result<_, Error>` petit (clippy::result_large_err).
    Api {
        status: u16,
        body: Box<ApiErrorBody>,
        request_id: Option<String>,
    },
    /// Réseau : DNS, TLS, délai dépassé…
    Transport(String),
    /// Réponse illisible.
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api { status, body, request_id } => {
                write!(f, "{} ({status})", body.message.as_deref().unwrap_or("API error"))?;
                if let Some(code) = &body.code {
                    write!(f, "\n  code:    {code}")?;
                }
                if let Some(kind) = &body.kind {
                    write!(f, "\n  type:    {kind}")?;
                }
                if let Some(param) = &body.param {
                    write!(f, "\n  param:   {param}")?;
                }
                if let Some(id) = request_id {
                    write!(f, "\n  request: {id}")?;
                }
                if let Some(url) = &body.doc_url {
                    write!(f, "\n  docs:    {url}")?;
                }
                Ok(())
            }
            Error::Transport(m) => write!(f, "could not reach the API: {m}"),
            Error::Decode(m) => write!(f, "unexpected response from the API: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl Client {
    pub fn new(base: &str, key: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .tls_config(tls_config())
            .try_proxy_from_env(true)
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("pulsesend-cli/", env!("CARGO_PKG_VERSION")))
            .build();

        Self {
            agent,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/{}", self.base, path.trim_start_matches('/'))
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        req.set("Authorization", &format!("Bearer {}", self.key))
            .set("Accept", "application/json")
    }

    pub fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, Error> {
        let mut req = self.auth(self.agent.get(&self.url(path)));
        for (k, v) in query {
            req = req.query(k, v);
        }

        Self::finish(req.call())
    }

    pub fn post(&self, path: &str, body: &Value, idempotency_key: Option<&str>) -> Result<Value, Error> {
        let mut req = self.auth(self.agent.post(&self.url(path)));
        if let Some(key) = idempotency_key {
            req = req.set("Idempotency-Key", key);
        }

        Self::finish(req.send_json(body))
    }

    fn finish(result: Result<ureq::Response, ureq::Error>) -> Result<Value, Error> {
        match result {
            Ok(resp) => resp.into_json::<Value>().map_err(|e| Error::Decode(e.to_string())),
            Err(ureq::Error::Status(status, resp)) => {
                let request_id = resp.header("X-Request-Id").map(str::to_string);
                let raw = resp.into_string().unwrap_or_default();
                let json = serde_json::from_str::<Value>(&raw).ok();
                // Format de l'API : { error: {...} }. Sinon (erreur hors API, proxy…), le message brut.
                let body = json
                    .as_ref()
                    .and_then(|v| v.get("error").cloned())
                    .and_then(|e| serde_json::from_value::<ApiErrorBody>(e).ok())
                    .unwrap_or_else(|| {
                        let message = json
                            .as_ref()
                            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                            .unwrap_or_else(|| {
                                if raw.trim().is_empty() {
                                    format!("HTTP {status}")
                                } else {
                                    truncate(raw.trim(), 200)
                                }
                            });

                        ApiErrorBody {
                            kind: None,
                            code: None,
                            message: Some(truncate(&message, 300)),
                            param: None,
                            doc_url: None,
                        }
                    });

                Err(Error::Api {
                    status,
                    body: Box::new(body),
                    request_id,
                })
            }
            Err(ureq::Error::Transport(t)) => Err(Error::Transport(t.to_string())),
        }
    }
}

/// Autorités de confiance : la liste intégrée (Mozilla), plus celles du système. Le CLI marche donc
/// sur une image minimale sans certificats, et derrière un proxy d'entreprise ou un CA local (mkcert).
fn tls_config() -> Arc<rustls::ClientConfig> {
    #[cfg_attr(target_os = "macos", allow(unused_mut))]
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    #[cfg(not(target_os = "macos"))]
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocoles TLS par défaut")
        .with_root_certificates(roots)
        .with_no_client_auth();

    Arc::new(config)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}
