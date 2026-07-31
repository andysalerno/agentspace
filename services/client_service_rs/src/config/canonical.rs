use sha2::{Digest, Sha256};

use crate::config::{
    document::{ConfigDocument, ConfigSpec},
    error::ConfigError,
};

/// Return the hex-encoded SHA-256 of the given bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Produce a copy of the spec with all identity-keyed resource collections
/// sorted deterministically. Order-significant fields inside a resource are
/// untouched.
#[must_use]
pub fn canonical_spec(spec: &ConfigSpec) -> ConfigSpec {
    let mut spec = spec.clone();
    spec.secrets.sort_by(|a, b| a.name.cmp(&b.name));
    spec.kernel_configs
        .sort_by(|a, b| a.harness.as_str().cmp(b.harness.as_str()));
    spec.connections.sort_by(|a, b| a.id.cmp(&b.id));
    spec.skills.sort_by(|a, b| a.id.cmp(&b.id));
    spec.agents.sort_by(|a, b| a.id.cmp(&b.id));
    spec.gateways.sort_by(|a, b| a.id.cmp(&b.id));
    spec
}

/// Produce a canonically ordered copy of the document.
#[must_use]
pub fn canonical_document(document: &ConfigDocument) -> ConfigDocument {
    ConfigDocument {
        metadata_name: document.metadata_name.clone(),
        spec: canonical_spec(&document.spec),
    }
}

/// Serialize the document into one deterministic, self-contained aggregate YAML
/// projection with stable resource ordering and LF line endings.
///
/// # Errors
/// Returns [`ConfigError::Serialize`] if the YAML emitter fails.
pub fn to_canonical_yaml(document: &ConfigDocument) -> Result<String, ConfigError> {
    let aggregate = canonical_document(document).into_aggregate();
    serde_yaml_ng::to_string(&aggregate).map_err(|error| ConfigError::Serialize {
        detail: error.to_string(),
    })
}

/// Compute the semantic SHA-256 from the canonical serialization. Used solely
/// for equality/no-op detection.
///
/// # Errors
/// Returns [`ConfigError::Serialize`] if canonical serialization fails.
pub fn semantic_hash(document: &ConfigDocument) -> Result<String, ConfigError> {
    Ok(sha256_hex(to_canonical_yaml(document)?.as_bytes()))
}

/// Collection-aware typed equality: two documents are equal when their
/// canonical (identity-sorted) forms are equal.
#[must_use]
pub fn typed_equal(left: &ConfigDocument, right: &ConfigDocument) -> bool {
    canonical_document(left) == canonical_document(right)
}
