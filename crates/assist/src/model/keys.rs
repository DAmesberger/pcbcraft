//! API keys: from the system keychain per connection, or from a named
//! environment variable for the CLI. A key lives in memory only; [`Secret`]
//! has no way to reach a file, a log or a recording by accident.
use super::Endpoint;
use crate::{Error, Result};

/// The keychain service every Pcbcraft model key is stored under; the entry
/// is the connection's base URL, so a key is only ever used for its endpoint.
const SERVICE: &str = "pcbcraft-llm";

/// A key in memory. It is not `Serialize`, and its `Debug` shows nothing.
#[derive(Clone)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl Secret {
    /// `None` when the text is blank.
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty()).then(|| Self(text.to_owned()))
    }

    /// The key, for the one request header that carries it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// `text` with every occurrence of the key removed.
    pub(crate) fn redact(&self, text: &str) -> String {
        text.replace(&self.0, "[redacted]")
    }
}

/// Where a connection's key comes from.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum KeySource {
    /// A local server that takes no key.
    #[default]
    None,
    /// A named environment variable (the CLI).
    Env(String),
    /// The system keychain's entry for this connection's endpoint.
    Keychain,
}

impl KeySource {
    /// The key, refused before it is read when the endpoint may not have
    /// one. A missing key names where it was looked for, never a value.
    pub fn resolve(&self, endpoint: &Endpoint) -> Result<Option<Secret>> {
        match self {
            Self::None => Ok(None),
            Self::Env(name) => {
                endpoint.check_key_allowed()?;
                let value = std::env::var(name).unwrap_or_default();
                Secret::new(&value)
                    .map(Some)
                    .ok_or_else(|| Error::Config(format!("{name} is not set")))
            }
            Self::Keychain => {
                endpoint.check_key_allowed()?;
                keychain_get(endpoint)?.map(Some).ok_or_else(|| {
                    Error::Config(format!(
                        "no key in the system keychain for {}",
                        endpoint.base()
                    ))
                })
            }
        }
    }
}

fn entry(endpoint: &Endpoint) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, endpoint.base())
        .map_err(|e| Error::Config(format!("the system keychain: {e}")))
}

fn keychain_get(endpoint: &Endpoint) -> Result<Option<Secret>> {
    match entry(endpoint)?.get_password() {
        Ok(text) => Ok(Secret::new(&text)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Config(format!("the system keychain: {e}"))),
    }
}

/// Keep `key` in the system keychain for this connection only. Refused for
/// an endpoint the key could never be sent to.
pub fn keychain_store(endpoint: &Endpoint, key: &Secret) -> Result<()> {
    endpoint.check_key_allowed()?;
    entry(endpoint)?
        .set_password(key.expose())
        .map_err(|e| Error::Config(format!("the system keychain: {e}")))
}

/// Remove this connection's key; whether there was one.
pub fn keychain_delete(endpoint: &Endpoint) -> Result<bool> {
    match entry(endpoint)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(Error::Config(format!("the system keychain: {e}"))),
    }
}

/// Whether this connection has a key, without reading it into anything else.
pub fn keychain_has(endpoint: &Endpoint) -> bool {
    matches!(keychain_get(endpoint), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints_and_redacts_itself() {
        let Some(secret) = Secret::new("  sk-test-123 ") else {
            unreachable_key();
            return;
        };
        assert_eq!(format!("{secret:?}"), "Secret([redacted])");
        assert_eq!(
            secret.redact("bad key sk-test-123 here"),
            "bad key [redacted] here"
        );
        assert!(Secret::new("   ").is_none());
    }

    fn unreachable_key() {
        assert_eq!("a key", "none");
    }

    #[test]
    fn a_key_is_refused_before_it_is_read_for_plain_http_to_another_computer() -> Result<()> {
        let far = Endpoint::parse("http://192.168.1.20:11434/v1")?;
        let refused = |source: KeySource| {
            source
                .resolve(&far)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        // The variable need not exist: the refusal comes first.
        assert!(refused(KeySource::Env("PCBCRAFT_NO_SUCH_VAR".into())).contains("plain HTTP"));
        // Refused before the keychain is touched.
        assert!(refused(KeySource::Keychain).contains("plain HTTP"));
        // A server without a key may be anywhere.
        assert!(KeySource::None.resolve(&far)?.is_none());
        Ok(())
    }

    #[test]
    fn a_missing_key_names_the_variable() -> Result<()> {
        let near = Endpoint::parse("http://localhost:1/v1")?;
        let e = KeySource::Env("PCBCRAFT_NO_SUCH_VAR".into()).resolve(&near);
        assert!(e.is_err_and(|e| e.to_string().contains("PCBCRAFT_NO_SUCH_VAR")));
        Ok(())
    }
}
