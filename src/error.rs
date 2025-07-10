// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

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
	/// Mortal transaction lifetime surpassed
	#[error("Mortal transaction lifetime surpassed, block number: {0}")]
	MortalLifetimeSurpassed(u64),
}
