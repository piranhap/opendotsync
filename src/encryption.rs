use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("encryption not yet implemented")]
    NotImplemented,
}

pub fn encrypt(data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    Ok(data.to_vec())
}

pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    Ok(data.to_vec())
}
