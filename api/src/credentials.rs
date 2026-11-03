//! Authenticated encryption for the reusable RustDesk personal address-book hash.
//!
//! Key material belongs in a separate, private key file, never in the database.
//! Keep previous key IDs configured while their stored credentials still exist.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Result, anyhow};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{
    Deserialize, Serialize,
    de::{MapAccess, Visitor},
};

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_KEYS: usize = 16;
const MAX_KEY_ID_BYTES: usize = 64;
const HASH_BYTES: usize = 32;
const HASH_BASE64_BYTES: usize = 44;
const NONCE_BYTES: usize = 12;
const CIPHERTEXT_BYTES: usize = HASH_BASE64_BYTES + aead::MAX_TAG_LEN;
const AAD_VERSION: &[u8] = b"rustdesk-account-api:peer-credential:aes-256-gcm:v1";

/// Only authenticated ciphertext and its public routing metadata are persisted.
/// Deliberately has no `Debug` implementation: this is still sensitive material.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedCredential {
    pub key_id: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// A bounded key ring with one active encryption key and optional old read keys.
/// Deliberately has no `Debug` implementation.
pub struct CredentialCipher {
    active_key: String,
    keys: BTreeMap<String, LessSafeKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyConfig {
    active_key: String,
    #[serde(deserialize_with = "unique_keys")]
    keys: BTreeMap<String, String>,
}

// Reject duplicate IDs instead of silently allowing the last JSON entry to win.
fn unique_keys<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct KeysVisitor;
    impl<'de> Visitor<'de> for KeysVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a bounded map of unique key IDs")
        }

        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut keys = BTreeMap::new();
            while let Some((id, value)) = map.next_entry::<String, String>()? {
                if keys.len() >= MAX_KEYS || keys.insert(id, value).is_some() {
                    return Err(serde::de::Error::custom("invalid key map"));
                }
            }
            Ok(keys)
        }
    }
    deserializer.deserialize_map(KeysVisitor)
}

fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_KEY_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Require the official standard-base64 SHA256(password || salt) representation.
/// Padded and unpadded forms are accepted and preserved verbatim by encryption.
/// This hash is itself a reusable credential; never apply another one-way hash.
pub fn validate_hash(hash: &str) -> Result<()> {
    if !matches!(hash.len(), 43 | HASH_BASE64_BYTES) {
        return Err(anyhow!("invalid credential hash"));
    }
    let engine = if hash.len() == HASH_BASE64_BYTES {
        &STANDARD
    } else {
        &STANDARD_NO_PAD
    };
    let decoded = engine
        .decode(hash)
        .map_err(|_| anyhow!("invalid credential hash"))?;
    if decoded.len() != HASH_BYTES || engine.encode(&decoded) != hash {
        return Err(anyhow!("invalid credential hash"));
    }
    Ok(())
}

impl CredentialCipher {
    pub fn from_json(json: &str) -> Result<Self> {
        let invalid = || anyhow!("invalid credential key configuration");
        if json.len() > MAX_CONFIG_BYTES {
            return Err(invalid());
        }
        let config: KeyConfig = serde_json::from_str(json).map_err(|_| invalid())?;
        if !valid_key_id(&config.active_key) || config.keys.is_empty() {
            return Err(invalid());
        }
        let mut keys = BTreeMap::new();
        for (id, encoded) in config.keys {
            if !valid_key_id(&id) || encoded.len() != HASH_BASE64_BYTES {
                return Err(invalid());
            }
            let key = STANDARD.decode(&encoded).map_err(|_| invalid())?;
            if key.len() != 32 || STANDARD.encode(&key) != encoded {
                return Err(invalid());
            }
            let key = UnboundKey::new(&aead::AES_256_GCM, &key).map_err(|_| invalid())?;
            keys.insert(id, LessSafeKey::new(key));
        }
        if !keys.contains_key(&config.active_key) {
            return Err(invalid());
        }
        Ok(Self {
            active_key: config.active_key,
            keys,
        })
    }

    /// Load a regular, non-symlink file with mode exactly 0600. No fixed UID is
    /// required, so the same policy works for an unprivileged container user.
    #[cfg(unix)]
    pub fn from_file(path: &Path) -> Result<Self> {
        use std::{
            fs::OpenOptions,
            io::Read,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };

        let invalid = || anyhow!("invalid credential key file");
        // O_NOFOLLOW closes the final-component symlink race. O_NONBLOCK avoids
        // blocking on a FIFO/device swapped in before the metadata check.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > MAX_CONFIG_BYTES as u64
        {
            return Err(invalid());
        }
        let mut json = String::new();
        file.take((MAX_CONFIG_BYTES + 1) as u64)
            .read_to_string(&mut json)
            .map_err(|_| invalid())?;
        if json.len() > MAX_CONFIG_BYTES {
            return Err(invalid());
        }
        Self::from_json(&json).map_err(|_| invalid())
    }

    #[cfg(not(unix))]
    pub fn from_file(_path: &Path) -> Result<Self> {
        Err(anyhow!("credential key file permissions unsupported"))
    }

    pub fn active_key_id(&self) -> &str {
        &self.active_key
    }

    pub fn has_key(&self, id: &str) -> bool {
        self.keys.contains_key(id)
    }

    pub fn validate_hash(hash: &str) -> Result<()> {
        validate_hash(hash)
    }

    pub fn seal(&self, owner: &str, peer: &str, hash: &str) -> Result<SealedCredential> {
        validate_hash(hash)?;
        let failed = || anyhow!("credential encryption failed");
        let key = self.keys.get(&self.active_key).ok_or_else(failed)?;
        let mut nonce = [0; NONCE_BYTES];
        SystemRandom::new().fill(&mut nonce).map_err(|_| failed())?;
        let aad = credential_aad(owner, peer, &self.active_key);
        let mut ciphertext = hash.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut ciphertext,
        )
        .map_err(|_| failed())?;
        Ok(SealedCredential {
            key_id: self.active_key.clone(),
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }

    pub fn open(&self, owner: &str, peer: &str, sealed: &SealedCredential) -> Result<String> {
        if !valid_key_id(&sealed.key_id) {
            return Err(anyhow!("credential decryption failed"));
        }
        let key = self
            .keys
            .get(&sealed.key_id)
            .ok_or_else(|| anyhow!("credential key unavailable"))?;
        let failed = || anyhow!("credential decryption failed");
        // Bound input before base64 decoding. These sizes follow from the fixed
        // hash representation and the AES-GCM nonce/tag sizes.
        if sealed.nonce.len() != 16 || sealed.ciphertext.len() != 80 {
            return Err(failed());
        }
        let nonce: [u8; NONCE_BYTES] = STANDARD
            .decode(&sealed.nonce)
            .map_err(|_| failed())?
            .try_into()
            .map_err(|_| failed())?;
        let mut ciphertext = STANDARD.decode(&sealed.ciphertext).map_err(|_| failed())?;
        if ciphertext.len() != CIPHERTEXT_BYTES && ciphertext.len() != CIPHERTEXT_BYTES - 1 {
            return Err(failed());
        }
        let aad = credential_aad(owner, peer, &sealed.key_id);
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut ciphertext,
            )
            .map_err(|_| failed())?;
        let hash = std::str::from_utf8(plaintext).map_err(|_| failed())?;
        validate_hash(hash).map_err(|_| failed())?;
        Ok(hash.to_owned())
    }
}

fn credential_aad(owner: &str, peer: &str, key_id: &str) -> Vec<u8> {
    let mut aad = Vec::new();
    // Every field is length-prefixed, so boundaries remain unambiguous even
    // with embedded NULs or separators in user-controlled owner/peer IDs.
    for field in [
        AAD_VERSION,
        owner.as_bytes(),
        peer.as_bytes(),
        key_id.as_bytes(),
    ] {
        aad.extend_from_slice(&(field.len() as u64).to_be_bytes());
        aad.extend_from_slice(field);
    }
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(active: &str, keys: &[(&str, u8)]) -> String {
        let keys: BTreeMap<_, _> = keys
            .iter()
            .map(|(id, byte)| (*id, STANDARD.encode([*byte; 32])))
            .collect();
        serde_json::json!({ "active_key": active, "keys": keys }).to_string()
    }

    fn hash() -> String {
        STANDARD.encode([42; 32])
    }

    #[test]
    fn roundtrip_uses_fresh_nonces_and_never_stores_hash() {
        let cipher = CredentialCipher::from_json(&config("v1", &[("v1", 1)])).unwrap();
        let first = cipher.seal("owner", "peer", &hash()).unwrap();
        let second = cipher.seal("owner", "peer", &hash()).unwrap();
        assert_eq!(cipher.open("owner", "peer", &first).unwrap(), hash());
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.ciphertext, second.ciphertext);
        assert!(!serde_json::to_string(&first).unwrap().contains(&hash()));
        assert_eq!(cipher.active_key_id(), "v1");
        assert!(cipher.has_key("v1"));
        let unpadded = hash().trim_end_matches('=').to_owned();
        let sealed = cipher.seal("owner", "peer", &unpadded).unwrap();
        assert_eq!(cipher.open("owner", "peer", &sealed).unwrap(), unpadded);
    }

    #[test]
    fn authentication_binds_owner_peer_key_id_and_ciphertext() {
        // Same key bytes under two IDs must still be bound to their ID by AAD.
        let cipher = CredentialCipher::from_json(&config("v1", &[("v1", 1), ("v2", 1)])).unwrap();
        let sealed = cipher.seal("ab", "c", &hash()).unwrap();
        assert!(cipher.open("a", "bc", &sealed).is_err());
        assert!(cipher.open("ab", "wrong-peer", &sealed).is_err());
        assert!(cipher.open("wrong-owner", "c", &sealed).is_err());
        let mut altered = sealed.clone();
        altered.key_id = "v2".into();
        assert!(cipher.open("ab", "c", &altered).is_err());
        let wrong = CredentialCipher::from_json(&config("v1", &[("v1", 2)])).unwrap();
        assert!(wrong.open("ab", "c", &sealed).is_err());
        let mut altered = sealed.clone();
        let mut ciphertext = STANDARD.decode(&altered.ciphertext).unwrap();
        ciphertext[0] ^= 1;
        altered.ciphertext = STANDARD.encode(ciphertext);
        assert!(cipher.open("ab", "c", &altered).is_err());
        let mut altered = sealed;
        altered.nonce = STANDARD.encode([0; NONCE_BYTES]);
        assert!(cipher.open("ab", "c", &altered).is_err());
    }

    #[test]
    fn rotation_reads_old_keys_and_encrypts_with_active_key() {
        let old = CredentialCipher::from_json(&config("v1", &[("v1", 1)])).unwrap();
        let sealed = old.seal("owner", "peer", &hash()).unwrap();
        let rotated = CredentialCipher::from_json(&config("v2", &[("v1", 1), ("v2", 2)])).unwrap();
        assert_eq!(rotated.open("owner", "peer", &sealed).unwrap(), hash());
        assert_eq!(rotated.seal("owner", "peer", &hash()).unwrap().key_id, "v2");
        let retired = CredentialCipher::from_json(&config("v2", &[("v2", 2)])).unwrap();
        assert!(!retired.has_key("v1"));
        assert_eq!(
            retired
                .open("owner", "peer", &sealed)
                .unwrap_err()
                .to_string(),
            "credential key unavailable"
        );
    }

    #[test]
    fn rejects_invalid_hashes_keys_ids_and_oversized_config() {
        for value in [
            "".to_owned(),
            "x".repeat(44),
            STANDARD.encode([1; 31]),
            STANDARD.encode([1; 33]),
        ] {
            assert!(validate_hash(&value).is_err());
        }
        for value in [
            "!".repeat(44),
            STANDARD.encode([1; 31]),
            STANDARD.encode([1; 33]),
        ] {
            let json =
                serde_json::json!({ "active_key": "v1", "keys": { "v1": value } }).to_string();
            assert!(CredentialCipher::from_json(&json).is_err());
        }
        for id in ["", "bad/id", "bad id", "密钥", &"x".repeat(65)] {
            assert!(CredentialCipher::from_json(&config(id, &[(id, 1)])).is_err());
        }
        assert!(CredentialCipher::from_json(&config("missing", &[("v1", 1)])).is_err());
        assert!(CredentialCipher::from_json(&" ".repeat(MAX_CONFIG_BYTES + 1)).is_err());
        let keys: Vec<_> = (0..=MAX_KEYS).map(|i| (format!("v{i}"), 1)).collect();
        let refs: Vec<_> = keys.iter().map(|(id, byte)| (id.as_str(), *byte)).collect();
        assert!(CredentialCipher::from_json(&config("v0", &refs)).is_err());
        let duplicate = format!(
            r#"{{"active_key":"v1","keys":{{"v1":"{}","v1":"{}"}}}}"#,
            STANDARD.encode([1; 32]),
            STANDARD.encode([2; 32])
        );
        assert!(CredentialCipher::from_json(&duplicate).is_err());
    }

    #[test]
    fn errors_do_not_expose_input_or_crypto_material() {
        let secret = "secret-must-never-appear";
        let invalid_json =
            format!(r#"{{"active_key":"{secret}","keys":{{"{secret}":"{secret}"}}}}"#);
        let error = CredentialCipher::from_json(&invalid_json).err().unwrap();
        assert_eq!(format!("{error:#}"), "invalid credential key configuration");
        assert!(!format!("{error:?}").contains(secret));
        let cipher = CredentialCipher::from_json(&config("v1", &[("v1", 1)])).unwrap();
        let error = cipher.seal(secret, secret, secret).err().unwrap();
        assert_eq!(format!("{error:#}"), "invalid credential hash");
        let sealed = SealedCredential {
            key_id: "v1".into(),
            nonce: secret.into(),
            ciphertext: secret.into(),
        };
        let error = cipher.open(secret, secret, &sealed).unwrap_err();
        assert_eq!(format!("{error:#}"), "credential decryption failed");
        assert!(!format!("{error:?}").contains(secret));
    }

    #[cfg(unix)]
    #[test]
    fn key_file_requires_private_regular_non_symlink_and_bounded_content() {
        use std::{
            fs,
            os::unix::fs::{PermissionsExt, symlink},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.json");
        fs::write(&path, config("v1", &[("v1", 1)])).unwrap();
        for mode in [0o644, 0o640, 0o400, 0o700, 0o1600] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(CredentialCipher::from_file(&path).is_err());
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(CredentialCipher::from_file(&path).is_ok());
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(CredentialCipher::from_file(&link).is_err());
        assert!(CredentialCipher::from_file(dir.path()).is_err());
        assert!(CredentialCipher::from_file(&dir.path().join("missing")).is_err());
        fs::write(&path, vec![b' '; MAX_CONFIG_BYTES + 1]).unwrap();
        assert!(CredentialCipher::from_file(&path).is_err());
        fs::write(&path, [0xff]).unwrap();
        assert!(CredentialCipher::from_file(&path).is_err());
    }
}
