#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Subxt error.
    #[error("subxt error: {0}")]
    Subxt(#[from] subxt::Error),
    /// Other error.
    #[error("Other error: {0}")]
    Other(String),
}
