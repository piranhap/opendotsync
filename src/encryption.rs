use std::io::{Read, Write};
use std::path::Path;

use thiserror::Error;

use crate::config::EncryptionConfig;

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("age error: {0}")]
    Age(String),
    #[error("no identity file configured for encryption")]
    NoIdentity,
}

/// Generate a new age x25519 keypair and write the identity file.
/// Returns the public key string ("age1...").
pub fn generate_keypair(output_path: &Path) -> Result<String, EncryptionError> {
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let pubkey = identity.to_public().to_string();
    let secret = identity.to_string();

    let content = format!(
        "# created: {}\n# public key: {pubkey}\n{}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        secret.expose_secret(),
    );

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, &content)?;
    Ok(pubkey)
}

/// Read the public key from an age identity file.
pub fn export_public_key(identity_path: &Path) -> Result<String, EncryptionError> {
    let content = std::fs::read_to_string(identity_path)?;
    for line in content.lines() {
        if let Some(key) = line.strip_prefix("# public key: ") {
            return Ok(key.trim().to_string());
        }
    }
    Ok(parse_identity(&content)?.to_public().to_string())
}

fn parse_identity(content: &str) -> Result<age::x25519::Identity, EncryptionError> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("AGE-SECRET-KEY-") {
            return line
                .parse::<age::x25519::Identity>()
                .map_err(|e| EncryptionError::Age(e.to_string()));
        }
    }
    Err(EncryptionError::Age(
        "no AGE-SECRET-KEY found in identity file".to_string(),
    ))
}

pub fn encrypt(data: &[u8], config: &EncryptionConfig) -> Result<Vec<u8>, EncryptionError> {
    if !config.enabled {
        return Ok(data.to_vec());
    }
    let identity_path = config
        .identity
        .as_ref()
        .ok_or(EncryptionError::NoIdentity)?;
    let content = std::fs::read_to_string(identity_path)?;
    let identity = parse_identity(&content)?;
    let recipient = identity.to_public();

    let encryptor =
        age::Encryptor::with_recipients(
            vec![Box::new(recipient) as Box<dyn age::Recipient + Send>],
        )
        .expect("non-empty recipients");

    let mut output = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut output)
        .map_err(|e| EncryptionError::Age(e.to_string()))?;
    writer.write_all(data)?;
    writer
        .finish()
        .map_err(|e| EncryptionError::Age(e.to_string()))?;

    Ok(output)
}

pub fn decrypt(data: &[u8], config: &EncryptionConfig) -> Result<Vec<u8>, EncryptionError> {
    if !config.enabled {
        return Ok(data.to_vec());
    }
    let identity_path = config
        .identity
        .as_ref()
        .ok_or(EncryptionError::NoIdentity)?;
    let content = std::fs::read_to_string(identity_path)?;
    let identity = parse_identity(&content)?;

    let decryptor = age::Decryptor::new(std::io::Cursor::new(data))
        .map_err(|e| EncryptionError::Age(e.to_string()))?;

    let mut reader = match decryptor {
        age::Decryptor::Recipients(d) => d
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|e| EncryptionError::Age(e.to_string()))?,
        age::Decryptor::Passphrase(_) => {
            return Err(EncryptionError::Age(
                "passphrase-encrypted data not supported".to_string(),
            ))
        }
    };

    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_enabled_config() {
        let dir = tempfile::tempdir().unwrap();
        let identity_path = dir.path().join("identity.age");
        generate_keypair(&identity_path).unwrap();

        let config = EncryptionConfig {
            enabled: true,
            identity: Some(identity_path),
        };
        let plaintext = b"export SECRET_TOKEN=value\n# multi-line\x00binary\xff";
        let ciphertext = encrypt(plaintext, &config).unwrap();
        assert_ne!(ciphertext, plaintext);
        let decrypted = decrypt(&ciphertext, &config).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn disabled_config_is_passthrough() {
        let config = EncryptionConfig {
            enabled: false,
            identity: None,
        };
        let data = b"plain bytes";
        assert_eq!(encrypt(data, &config).unwrap(), data);
        assert_eq!(decrypt(data, &config).unwrap(), data);
    }

    #[test]
    fn enabled_without_identity_errors() {
        let config = EncryptionConfig {
            enabled: true,
            identity: None,
        };
        assert!(matches!(
            encrypt(b"x", &config),
            Err(EncryptionError::NoIdentity)
        ));
    }

    #[test]
    fn export_public_key_matches_generated() {
        let dir = tempfile::tempdir().unwrap();
        let identity_path = dir.path().join("identity.age");
        let pubkey = generate_keypair(&identity_path).unwrap();
        assert_eq!(export_public_key(&identity_path).unwrap(), pubkey);
    }
}
