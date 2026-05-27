use thiserror::Error;

#[derive(Debug, Error)]
pub enum RetentionError {
    #[error("retention not yet implemented")]
    NotImplemented,
}

pub fn prune(_profile: &str, _keep: usize) -> Result<(), RetentionError> {
    Ok(())
}
