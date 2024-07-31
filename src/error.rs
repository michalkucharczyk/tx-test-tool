use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum Error {
	/// Subxt error.
	#[serde(skip)]
	#[error("subxt error: {0}")]
	Subxt(#[from] subxt::Error),
	/// Other error.
	#[error("Other error: {0}")]
	Other(String),
}
