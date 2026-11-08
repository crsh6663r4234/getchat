//! Versioned key-possession proofs. This module does not enroll devices or
//! attest binaries. Supply a binding obtained from an authenticated trust store.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{collections::BTreeSet, fmt, time::Duration};

const DOMAIN: &[u8] = b"GhostChat.DeviceProof\0";
const MAX_PROOF_AGE_MS: u64 = 90_000;
const MAX_FUTURE_SKEW_MS: u64 = 30_000;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestProof {
    pub version: u16,
    pub audience: String,
    pub account_id: String,
    pub device_id: String,
    pub session_id: String,
    pub method: String,
    pub target: String,
    pub body_sha256: String,
    pub timestamp_ms: u64,
    pub request_id: String,
    pub signature: String,
}

pub struct TrustedBinding<'a> {
    pub audience: &'a str,
    pub account_id: &'a str,
    pub device_id: &'a str,
    pub session_id: &'a str,
    pub verifying_key: &'a [u8; 32],
    pub revoked: bool,
}

/// Digest of the exact received body, computed by caller under a bounded body
/// limit or a separately authenticated upload-manifest protocol.
pub struct RequestContext<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub body_sha256: &'a [u8; 32],
    pub now_ms: u64,
}

pub struct VerifiedProof {
    pub request_id: String,
    /// Namespace an atomic replay claim by this trusted account/device/session
    /// binding. Caller must claim before authorizing/performing the operation.
    pub replay_binding: String,
    pub replay_ttl: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofError {
    InvalidShape,
    WrongBinding,
    WrongRequest,
    Expired,
    InvalidSignature,
}
impl fmt::Display for ProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Device request proof was rejected")
    }
}
impl std::error::Error for ProofError {}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
}

fn canonical_target(target: &str) -> bool {
    if target.len() > 2048
        || !target.starts_with('/')
        || target.starts_with("//")
        || !target.is_ascii()
        || target
            .bytes()
            .any(|b| b <= 0x20 || b == 0x7f || b == b'\\' || b == b'#')
    {
        return false;
    }
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(a, b)| (a, Some(b)));
    if path.contains("//") || path.split('/').any(|part| part == "." || part == "..") {
        return false;
    }
    // Encoded path delimiters, percent and dots could be interpreted differently
    // by proxies/frameworks. This protocol version rejects those aliases.
    let bytes = target.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1..index + 3]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
            {
                return false;
            }
            let decoded = u8::from_str_radix(&target[index + 1..index + 3], 16).unwrap_or(0);
            if decoded <= 0x20 && decoded != b' '
                || decoded == 0x7f
                || decoded.is_ascii_alphanumeric()
                || b"-_.~".contains(&decoded)
            {
                return false;
            }
            if index < path.len() && b"/\\%".contains(&decoded) {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    if let Some(query) = query {
        if query.is_empty() || query.contains('?') {
            return false;
        }
        let mut keys = BTreeSet::new();
        let mut previous = None;
        for pair in query.split('&') {
            let Some((key, _)) = pair.split_once('=') else {
                return false;
            };
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                || !keys.insert(key)
            {
                return false;
            }
            if previous.is_some_and(|last: &str| last >= key) {
                return false;
            }
            previous = Some(key);
        }
    }
    true
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend_from_slice(value);
}

impl RequestProof {
    /// DOMAIN || u16be(version) || length-prefixed UTF-8 audience/account/device/
    /// session/method/target || raw SHA256[32] || u64be(timestamp_ms) || UUID[16].
    /// Signature is Ed25519 over these bytes (not over serialized JSON).
    pub fn signing_bytes(&self) -> Result<Vec<u8>, ProofError> {
        if self.version != 1
            || !identifier(&self.audience)
            || !identifier(&self.account_id)
            || !identifier(&self.device_id)
            || !identifier(&self.session_id)
            || !matches!(
                self.method.as_str(),
                "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
            )
            || !canonical_target(&self.target)
        {
            return Err(ProofError::InvalidShape);
        }
        let digest = URL_SAFE_NO_PAD
            .decode(&self.body_sha256)
            .map_err(|_| ProofError::InvalidShape)?;
        if digest.len() != 32 || URL_SAFE_NO_PAD.encode(&digest) != self.body_sha256 {
            return Err(ProofError::InvalidShape);
        }
        let id = uuid::Uuid::parse_str(&self.request_id).map_err(|_| ProofError::InvalidShape)?;
        if id.to_string() != self.request_id {
            return Err(ProofError::InvalidShape);
        }
        let mut bytes = Vec::with_capacity(512);
        bytes.extend_from_slice(DOMAIN);
        bytes.extend_from_slice(&self.version.to_be_bytes());
        for value in [
            &self.audience,
            &self.account_id,
            &self.device_id,
            &self.session_id,
            &self.method,
            &self.target,
        ] {
            append_field(&mut bytes, value.as_bytes());
        }
        bytes.extend_from_slice(&digest);
        bytes.extend_from_slice(&self.timestamp_ms.to_be_bytes());
        bytes.extend_from_slice(id.as_bytes());
        Ok(bytes)
    }
}

pub fn verify(
    proof: &RequestProof,
    binding: &TrustedBinding<'_>,
    request: &RequestContext<'_>,
) -> Result<VerifiedProof, ProofError> {
    if binding.revoked
        || proof.audience != binding.audience
        || proof.account_id != binding.account_id
        || proof.device_id != binding.device_id
        || proof.session_id != binding.session_id
    {
        return Err(ProofError::WrongBinding);
    }
    if proof.method != request.method
        || proof.target != request.target
        || proof.body_sha256 != URL_SAFE_NO_PAD.encode(request.body_sha256)
    {
        return Err(ProofError::WrongRequest);
    }
    if proof.timestamp_ms > request.now_ms.saturating_add(MAX_FUTURE_SKEW_MS)
        || request.now_ms > proof.timestamp_ms.saturating_add(MAX_PROOF_AGE_MS)
    {
        return Err(ProofError::Expired);
    }
    let bytes = proof.signing_bytes()?;
    let signature = URL_SAFE_NO_PAD
        .decode(&proof.signature)
        .map_err(|_| ProofError::InvalidSignature)?;
    if signature.len() != 64 || URL_SAFE_NO_PAD.encode(&signature) != proof.signature {
        return Err(ProofError::InvalidSignature);
    }
    let signature = Signature::from_slice(&signature).map_err(|_| ProofError::InvalidSignature)?;
    let key = VerifyingKey::from_bytes(binding.verifying_key)
        .map_err(|_| ProofError::InvalidSignature)?;
    key.verify_strict(&bytes, &signature)
        .map_err(|_| ProofError::InvalidSignature)?;
    Ok(VerifiedProof {
        request_id: proof.request_id.clone(),
        replay_binding: format!(
            "{}|{}|{}|{}",
            binding.audience, binding.account_id, binding.device_id, binding.session_id
        ),
        replay_ttl: Duration::from_millis(MAX_PROOF_AGE_MS + MAX_FUTURE_SKEW_MS + 1000),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    fn fixture() -> (RequestProof, SigningKey, [u8; 32]) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let body: [u8; 32] = Sha256::digest(b"hello").into();
        let mut proof = RequestProof {
            version: 1,
            audience: "deployment-1".into(),
            account_id: "account-1".into(),
            device_id: "device-1".into(),
            session_id: "session-1".into(),
            method: "POST".into(),
            target: "/v1/test?a=1&b=two%20words".into(),
            body_sha256: URL_SAFE_NO_PAD.encode(body),
            timestamp_ms: 1_000_000,
            request_id: "00000000-0000-4000-8000-000000000001".into(),
            signature: String::new(),
        };
        proof.signature =
            URL_SAFE_NO_PAD.encode(key.sign(&proof.signing_bytes().unwrap()).to_bytes());
        (proof, key, body)
    }
    #[test]
    fn valid_proof_is_bound_to_key_session_body_target_and_time() {
        let (proof, key, body) = fixture();
        let public = key.verifying_key().to_bytes();
        let mut binding = TrustedBinding {
            audience: "deployment-1",
            account_id: "account-1",
            device_id: "device-1",
            session_id: "session-1",
            verifying_key: &public,
            revoked: false,
        };
        let mut request = RequestContext {
            method: "POST",
            target: &proof.target,
            body_sha256: &body,
            now_ms: 1_000_050,
        };
        let verified = verify(&proof, &binding, &request).unwrap();
        assert_eq!(verified.replay_ttl, Duration::from_secs(121));
        binding.session_id = "another-session";
        assert!(verify(&proof, &binding, &request).is_err());
        binding.session_id = "session-1";
        binding.audience = "other-deployment";
        assert!(verify(&proof, &binding, &request).is_err());
        binding.audience = "deployment-1";
        binding.revoked = true;
        assert!(verify(&proof, &binding, &request).is_err());
        binding.revoked = false;
        request.target = "/v1/different";
        assert!(verify(&proof, &binding, &request).is_err());
        request.target = &proof.target;
        let wrong = [0; 32];
        request.body_sha256 = &wrong;
        assert!(verify(&proof, &binding, &request).is_err());
        request.body_sha256 = &body;
        request.now_ms = 1_090_001;
        assert!(verify(&proof, &binding, &request).is_err());
        request.now_ms = 969_999;
        assert!(verify(&proof, &binding, &request).is_err());
        request.now_ms = 1_000_000;
        let mut tampered = proof.clone();
        tampered.request_id = "00000000-0000-4000-8000-000000000002".into();
        assert!(verify(&tampered, &binding, &request).is_err());
    }
    #[test]
    fn canonical_target_rejects_ambiguous_aliases_and_duplicate_query_keys() {
        for target in [
            "https://host/v1",
            "//host/v1",
            "/a/../b",
            "/a/%2E%2E/b",
            "/a%2Fb",
            "/a%5cb",
            "/a%252Fb",
            "/test?a=1&a=2",
            "/test?b=1&a=2",
            "/test?%61=1",
            "/test?key",
            "/test#fragment",
            "/test?q=%0A",
        ] {
            assert!(!canonical_target(target), "accepted {target}");
        }
        assert!(canonical_target("/v1/messages?q=two%20words"));
    }
    #[test]
    fn encoding_is_deterministic_and_unambiguous() {
        let (proof, _, _) = fixture();
        let bytes = proof.signing_bytes().unwrap();
        assert!(bytes.starts_with(b"GhostChat.DeviceProof\0\0\x01\0\0\0\x0cdeployment-1"));
        let roundtrip:RequestProof=serde_json::from_value(serde_json::json!({"signature":proof.signature,"request_id":proof.request_id,"timestamp_ms":proof.timestamp_ms,"body_sha256":proof.body_sha256,"target":proof.target,"method":proof.method,"session_id":proof.session_id,"device_id":proof.device_id,"account_id":proof.account_id,"audience":proof.audience,"version":proof.version})).unwrap();
        assert_eq!(bytes, roundtrip.signing_bytes().unwrap());
    }
}
