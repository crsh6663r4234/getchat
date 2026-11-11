//! Identity capability backup only. Never accepts MLS state or a storage DEK.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use blahaj::{Share, Sharks};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    version: u8,
    account_id: String,
    backup_id: String,
    public_key: String,
    nonce: String,
    encrypted_identity_capability: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryShare {
    backup_id: String,
    share: String,
    signature: String,
}

#[derive(Serialize)]
pub struct RecoveryKit {
    pub manifest: Manifest,
    pub shares: Vec<RecoveryShare>,
    /// Must be saved/verified separately; reading it from the kit is not trust.
    pub external_anchor: String,
}

fn aad(account: &str, backup: &str) -> Vec<u8> {
    format!("GhostChat/identity-recovery/v1\0{account}\0{backup}").into_bytes()
}

fn signed_share(manifest: &Manifest, share: &str) -> Result<Vec<u8>, String> {
    let mut bytes = b"GhostChat/recovery-share/v1\0".to_vec();
    let encoded = serde_json::to_vec(manifest).map_err(|_| "Invalid recovery manifest")?;
    bytes.extend_from_slice(&Sha256::digest(encoded));
    bytes.extend_from_slice(share.as_bytes());
    Ok(bytes)
}

pub fn split(account: &str, capability: &str) -> Result<RecoveryKit, String> {
    let account = Uuid::parse_str(account)
        .map_err(|_| "Invalid account ID")?
        .to_string();
    if !(32..=128).contains(&capability.len()) || !capability.is_ascii() {
        return Err("Invalid account recovery capability".into());
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    OsRng.fill_bytes(&mut *key);
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let backup_id = Uuid::new_v4().to_string();
    let signing = SigningKey::generate(&mut OsRng);
    let public_key = signing.verifying_key().to_bytes();
    let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| "Invalid recovery key")?;
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: capability.as_bytes(),
                aad: &aad(&account, &backup_id),
            },
        )
        .map_err(|_| "Could not encrypt identity recovery")?;
    let manifest = Manifest {
        version: 1,
        account_id: account,
        backup_id: backup_id.clone(),
        public_key: B64.encode(public_key),
        nonce: B64.encode(nonce),
        encrypted_identity_capability: B64.encode(encrypted),
    };
    let shares = Sharks(3)
        .dealer(&*key)
        .take(5)
        .map(|s| {
            let raw = Zeroizing::new(Vec::<u8>::from(&s));
            let share = B64.encode(&*raw);
            let signature = B64.encode(signing.sign(&signed_share(&manifest, &share)?).to_bytes());
            Ok(RecoveryShare {
                backup_id: backup_id.clone(),
                share,
                signature,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RecoveryKit {
        manifest,
        shares,
        external_anchor: B64.encode(Sha256::digest(public_key)),
    })
}

pub fn recover(
    manifest: &Manifest,
    shares: &[RecoveryShare],
    account: &str,
    external_anchor: &str,
) -> Result<Zeroizing<String>, String> {
    let account = Uuid::parse_str(account)
        .map_err(|_| "Invalid account ID")?
        .to_string();
    if manifest.version != 1
        || manifest.account_id != account
        || !(3..=5).contains(&shares.len())
        || manifest.encrypted_identity_capability.len() > 256
        || manifest.public_key.len() > 64
    {
        return Err("Invalid identity recovery request".into());
    }
    let key_bytes: [u8; 32] = B64
        .decode(&manifest.public_key)
        .map_err(|_| "Invalid recovery identity")?
        .try_into()
        .map_err(|_| "Invalid recovery identity")?;
    if B64.encode(Sha256::digest(key_bytes)) != external_anchor {
        return Err("Recovery identity does not match the separately saved anchor".into());
    }
    let verify = VerifyingKey::from_bytes(&key_bytes).map_err(|_| "Invalid recovery identity")?;
    let mut parsed = Vec::new();
    let mut indices = std::collections::HashSet::new();
    for share in shares {
        if share.backup_id != manifest.backup_id
            || share.share.len() > 64
            || share.signature.len() > 96
        {
            return Err("Mixed or invalid recovery shares".into());
        }
        let sig_bytes = B64
            .decode(&share.signature)
            .map_err(|_| "Invalid share signature")?;
        let signature = Signature::from_slice(&sig_bytes).map_err(|_| "Invalid share signature")?;
        verify
            .verify_strict(&signed_share(manifest, &share.share)?, &signature)
            .map_err(|_| "Recovery share authentication failed")?;
        let raw = Zeroizing::new(
            B64.decode(&share.share)
                .map_err(|_| "Invalid recovery share")?,
        );
        if raw.len() != 33 || raw[0] == 0 || !indices.insert(raw[0]) {
            return Err("Duplicate or invalid recovery share".into());
        }
        parsed.push(Share::try_from(raw.as_slice()).map_err(|_| "Invalid recovery share")?);
    }
    let key = Zeroizing::new(
        Sharks(3)
            .recover(&parsed)
            .map_err(|_| "Not enough valid recovery shares")?,
    );
    let nonce = B64
        .decode(&manifest.nonce)
        .map_err(|_| "Invalid recovery nonce")?;
    if nonce.len() != 12 {
        return Err("Invalid recovery nonce".into());
    }
    let encrypted = B64
        .decode(&manifest.encrypted_identity_capability)
        .map_err(|_| "Invalid recovery envelope")?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "Invalid recovered key")?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &encrypted,
                    aad: &aad(&account, &manifest.backup_id),
                },
            )
            .map_err(|_| "Identity recovery authentication failed")?,
    );
    let result =
        String::from_utf8(plaintext.to_vec()).map_err(|_| "Invalid identity capability")?;
    Ok(Zeroizing::new(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Shares written by earlier builds (sharks 0.5) must still recover.
    #[test]
    fn shares_from_earlier_builds_still_recover() {
        let shares: Vec<Share> = [
            "ARwPkqMqfUz5nB1Z2VnkqgluMsqKCQ2XmT6QI1BYmB-2",
            "ApS94sLQfYivVZhpD_ugs6K_yZhjaV7Yn6Wm_deMzCoL",
            "A4mwc2X_BsNewI872q9KFrvA6UH9dUVYHoIsxZvJSiqd",
        ]
        .iter()
        .map(|s| Share::try_from(B64.decode(s).unwrap().as_slice()).unwrap())
        .collect();
        let secret: Vec<u8> = (1..=32).collect();
        assert_eq!(Sharks(3).recover(&shares).unwrap(), secret);
    }

    /// RUSTSEC-2024-0398: every polynomial coefficient must be drawn from
    /// the whole field, zero included. With an RNG that only yields zeros
    /// the polynomial is the constant secret, so every share equals it.
    #[test]
    fn split_coefficients_may_be_zero() {
        struct Zeros;
        impl RngCore for Zeros {
            fn next_u32(&mut self) -> u32 {
                0
            }
            fn next_u64(&mut self) -> u64 {
                0
            }
            fn fill_bytes(&mut self, dest: &mut [u8]) {
                dest.fill(0);
            }
            fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
                dest.fill(0);
                Ok(())
            }
        }
        let secret: Vec<u8> = (1..=32).collect();
        for share in Sharks(3).dealer_rng(&secret, &mut Zeros).take(5) {
            assert_eq!(Vec::<u8>::from(&share)[1..], secret[..]);
        }
    }

    #[test]
    fn all_ten_subsets_restore_only_identity_and_reject_mixed_or_corrupt_shares() {
        let account = Uuid::new_v4().to_string();
        let capability = "a".repeat(64);
        let kit = split(&account, &capability).unwrap();
        for i in 0..5 {
            for j in i + 1..5 {
                for k in j + 1..5 {
                    let chosen = [&kit.shares[i], &kit.shares[j], &kit.shares[k]];
                    let chosen: Vec<RecoveryShare> = chosen
                        .iter()
                        .map(|s| serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap())
                        .collect();
                    assert_eq!(
                        &**recover(&kit.manifest, &chosen, &account, &kit.external_anchor).unwrap(),
                        capability
                    );
                }
            }
        }
        assert!(recover(
            &kit.manifest,
            &kit.shares[..2],
            &account,
            &kit.external_anchor
        )
        .is_err());
        assert!(recover(&kit.manifest, &kit.shares[..3], &account, "untrusted").is_err());
        let other = split(&account, &capability).unwrap();
        assert!(recover(
            &kit.manifest,
            &other.shares[..3],
            &account,
            &kit.external_anchor
        )
        .is_err());
        let mut corrupt: Vec<RecoveryShare> =
            serde_json::from_value(serde_json::to_value(&kit.shares[..3]).unwrap()).unwrap();
        corrupt[0].share.replace_range(..1, "!");
        assert!(recover(&kit.manifest, &corrupt, &account, &kit.external_anchor).is_err());
    }
}
