//! Request signing for Revolut Business API keys. A key (`r_live_...` or `r_test_...`) is sent as
//! `Authorization: Bearer <key>`, and every request also carries:
//!
//! - `X-Revx-Timestamp`: the time in Unix milliseconds;
//! - `X-Revx-Signature`: the Ed25519 signature, in standard base64, of the timestamp, the HTTP
//!   method in capitals, the path (from `/api`), the query string without its `?`, and the body
//!   exactly as sent, joined with no separators.
//!
//! The key pair is the one whose public key was registered with the API key in Revolut Business
//! (Settings, APIs).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TIMESTAMP_HEADER: &str = "X-Revx-Timestamp";
pub const SIGNATURE_HEADER: &str = "X-Revx-Signature";

/// Signs requests with the API key's Ed25519 private key.
pub struct Signer {
    key: Ed25519KeyPair,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").finish_non_exhaustive()
    }
}

/// The headers that sign one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub timestamp: String,
    pub signature: String,
}

impl Signer {
    /// A signer from a PEM Ed25519 private key (`BEGIN PRIVATE KEY`, as
    /// `openssl genpkey -algorithm ed25519` writes it).
    pub fn from_pem(pem: &str) -> Result<Self, String> {
        let (label, der) = pem_block(pem)?;
        match label.as_str() {
            "PRIVATE KEY" => {}
            "ENCRYPTED PRIVATE KEY" => {
                return Err(
                    "the private key is encrypted; give it without a passphrase (openssl pkey -in key.pem -out plain.pem)"
                        .into(),
                );
            }
            other => {
                return Err(format!(
                    "expected an Ed25519 private key in PEM (BEGIN PRIVATE KEY), found BEGIN {other}"
                ));
            }
        }
        let key = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| {
            "the private key is not an Ed25519 key; Revolut Business API keys use Ed25519 (openssl genpkey -algorithm ed25519)"
                .to_string()
        })?;
        Ok(Signer { key })
    }

    /// The public key, 32 raw bytes, for checking signatures in tests.
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }

    /// The signature of a request at `timestamp_ms`: `path` from `/api`, `query` without `?`.
    pub fn sign_at(
        &self,
        timestamp_ms: u128,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> Signature {
        let timestamp = timestamp_ms.to_string();
        let mut message =
            Vec::with_capacity(timestamp.len() + path.len() + query.len() + body.len() + 8);
        message.extend_from_slice(timestamp.as_bytes());
        message.extend_from_slice(method.to_ascii_uppercase().as_bytes());
        message.extend_from_slice(path.as_bytes());
        message.extend_from_slice(query.as_bytes());
        message.extend_from_slice(body);
        Signature {
            timestamp,
            signature: STANDARD.encode(self.key.sign(&message)),
        }
    }

    /// The signature of a request now.
    pub fn sign(&self, method: &str, path: &str, query: &str, body: &[u8]) -> Signature {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.sign_at(now, method, path, query, body)
    }
}

/// The label and DER bytes of the first PEM block in `pem`. Tolerates `\n` written literally, as
/// a key pasted into a single-line environment variable often is.
fn pem_block(pem: &str) -> Result<(String, Vec<u8>), String> {
    let pem = pem.replace("\\n", "\n");
    let start = pem
        .find("-----BEGIN ")
        .ok_or("expected a PEM private key (-----BEGIN PRIVATE KEY----- ...)")?;
    let rest = &pem[start + "-----BEGIN ".len()..];
    let (label, rest) = rest
        .split_once("-----")
        .ok_or("the PEM BEGIN line is malformed")?;
    let end_marker = format!("-----END {label}-----");
    let body = rest
        .split_once(&end_marker)
        .map(|(b, _)| b)
        .ok_or_else(|| format!("the PEM block has no {end_marker} line"))?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = STANDARD
        .decode(b64)
        .map_err(|_| "the PEM block is not valid base64".to_string())?;
    Ok((label.to_string(), der))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ED25519, UnparsedPublicKey};

    const KEY: &str = include_str!("../tests/fixtures/test-ed25519.pem");
    const PUBLIC: &str = include_str!("../tests/fixtures/test-ed25519-public.pem");

    #[test]
    fn signatures_cover_timestamp_method_path_query_and_body() {
        let s = Signer::from_pem(KEY).unwrap();
        let sig = s.sign_at(
            1_794_225_494_000,
            "post",
            "/api/1.0/pay",
            "a=1&b=2",
            br#"{"x":1}"#,
        );
        assert_eq!(sig.timestamp, "1794225494000");
        let raw = STANDARD.decode(&sig.signature).unwrap();
        let public = UnparsedPublicKey::new(&ED25519, s.public_key());
        public
            .verify(br#"1794225494000POST/api/1.0/paya=1&b=2{"x":1}"#, &raw)
            .expect("signature verifies");
        assert!(
            public
                .verify(b"1794225494000POST/api/1.0/pay", &raw)
                .is_err()
        );
    }

    #[test]
    fn the_public_key_matches_the_registered_one() {
        let s = Signer::from_pem(KEY).unwrap();
        let (_, der) = pem_block(PUBLIC).unwrap();
        // SubjectPublicKeyInfo for Ed25519 ends with the 32 raw key bytes.
        assert_eq!(&der[der.len() - 32..], s.public_key());
    }

    #[test]
    fn keys_pasted_on_one_line_are_read() {
        Signer::from_pem(&KEY.trim().replace('\n', "\\n")).unwrap();
    }

    #[test]
    fn unusable_keys_say_why() {
        let e = |pem: &str| Signer::from_pem(pem).unwrap_err().to_lowercase();
        assert!(e("not a key").contains("pem"));
        assert!(e(PUBLIC).contains("public key"));
        assert!(
            e("-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----")
                .contains("encrypted")
        );
        assert!(
            e("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").contains("ed25519")
        );
        assert!(e("-----BEGIN PRIVATE KEY-----\nAAAA").contains("end"));
    }

    #[test]
    fn the_key_never_shows_in_debug() {
        assert_eq!(
            format!("{:?}", Signer::from_pem(KEY).unwrap()),
            "Signer { .. }"
        );
    }
}
