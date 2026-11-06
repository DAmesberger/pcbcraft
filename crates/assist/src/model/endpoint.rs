//! A model endpoint's URL, and the one place that decides whether a key may
//! be sent to it: over HTTPS, or over plain HTTP only to this computer.
use crate::{Error, Result};
use std::net::IpAddr;

/// Longest endpoint URL accepted.
const MAX_URL: usize = 2048;

/// A checked base URL with no credentials, query or fragment in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    base: String,
    https: bool,
    loopback: bool,
}

impl Endpoint {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let bad = |why: &str| Error::Config(format!("endpoint {text:?}: {why}"));
        if text.len() > MAX_URL || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(bad("not a URL"));
        }
        let uri: ureq::http::Uri = text.parse().map_err(|_| bad("not a URL"))?;
        let https = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err(bad("it must start with http:// or https://")),
        };
        let Some(authority) = uri.authority() else {
            return Err(bad("it has no host"));
        };
        if authority.as_str().contains('@') {
            return Err(bad("credentials do not belong in the URL"));
        }
        if uri.query().is_some() || text.contains('#') {
            return Err(bad("a query or fragment does not belong in the base URL"));
        }
        Ok(Self {
            base: text.trim_end_matches('/').to_owned(),
            https,
            loopback: is_loopback_host(authority.host()),
        })
    }

    /// The base URL, with no trailing slash. It identifies the connection.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    /// Whether requests stay on this computer, for showing where data goes.
    pub fn location(&self) -> &'static str {
        if self.loopback {
            "this computer"
        } else if self.https {
            "another computer, over HTTPS"
        } else {
            "another computer, unencrypted"
        }
    }

    /// Refuse a key over plain HTTP to another computer.
    pub fn check_key_allowed(&self) -> Result<()> {
        if self.https || self.loopback {
            Ok(())
        } else {
            Err(Error::Config(format!(
                "refusing to send an API key to {} over plain HTTP: use https://, or an endpoint on this computer",
                self.base
            )))
        }
    }
}

/// `localhost` itself or a loopback address, by exact parse: a name such as
/// `localhost.example.com` or `127.0.0.1.example.com` is another computer.
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>()
        .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_computer_counts_as_loopback() -> Result<()> {
        for url in [
            "http://localhost:11434/v1",
            "http://LOCALHOST/v1",
            "http://127.0.0.1:8080",
            "http://127.1.2.3/v1",
            "http://[::1]:8080/v1",
            "http://[::ffff:127.0.0.1]/v1",
        ] {
            assert!(Endpoint::parse(url)?.is_loopback(), "{url}");
            assert!(Endpoint::parse(url)?.check_key_allowed().is_ok(), "{url}");
        }
        for url in [
            "http://localhost.example.com/v1",
            "http://127.0.0.1.example.com/v1",
            "http://192.168.1.20:11434/v1",
            "http://0.0.0.0:11434/v1",
            "http://example.com/v1",
        ] {
            assert!(!Endpoint::parse(url)?.is_loopback(), "{url}");
            assert!(Endpoint::parse(url)?.check_key_allowed().is_err(), "{url}");
        }
        assert!(
            Endpoint::parse("https://api.example.com/v1")?
                .check_key_allowed()
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn bad_urls_are_refused_and_a_trailing_slash_is_dropped() -> Result<()> {
        for url in [
            "",
            "localhost:11434/v1",
            "ftp://example.com",
            "http://user:secret@example.com/v1",
            "http://localhost@example.com/v1",
            "http://example.com/v1?key=abc",
            "http://example.com/v1#frag",
            "http://exa mple.com",
        ] {
            assert!(Endpoint::parse(url).is_err(), "{url}");
        }
        assert_eq!(
            Endpoint::parse("http://localhost:11434/v1/")?.base(),
            "http://localhost:11434/v1"
        );
        Ok(())
    }

    #[test]
    fn errors_do_not_repeat_credentials_beyond_the_url_given() {
        // The refusal for a key over HTTP names the endpoint, never a key.
        let refused = Endpoint::parse("http://example.com/v1")
            .and_then(|e| e.check_key_allowed())
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(refused.contains("plain HTTP"), "{refused}");
    }
}
