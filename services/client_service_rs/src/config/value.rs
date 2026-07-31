use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeMap};

/// Error raised when a secret name does not match the required grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidSecretName {
    pub value: String,
}

impl Display for InvalidSecretName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "secret name must match [A-Z][A-Z0-9_]*, got {:?}",
            self.value
        )
    }
}

impl std::error::Error for InvalidSecretName {}

/// A validated, immutable secret declaration name.
///
/// Names are case-sensitive and use `[A-Z][A-Z0-9_]*` so they are portable
/// across the UI, CLI, and providers.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SecretName(String);

impl SecretName {
    /// Validate and construct a [`SecretName`].
    ///
    /// # Errors
    /// Returns [`InvalidSecretName`] when `value` does not match `[A-Z][A-Z0-9_]*`.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSecretName> {
        let value = value.into();
        if Self::is_valid(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidSecretName { value })
        }
    }

    #[must_use]
    pub fn is_valid(value: &str) -> bool {
        let mut bytes = value.bytes();
        match bytes.next() {
            Some(first) if first.is_ascii_uppercase() => {}
            _ => return false,
        }
        bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Display for SecretName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for SecretName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// A configurable scalar leaf that is either a literal value of type `T` or an
/// explicit, unambiguous secret reference.
///
/// The YAML-native object form (`{ secretRef: NAME }`) is used rather than a
/// magic string so it can never collide with a legitimate literal value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigValue<T> {
    Literal(T),
    Secret(SecretName),
}

impl<T> ConfigValue<T> {
    #[must_use]
    pub const fn as_literal(&self) -> Option<&T> {
        match self {
            Self::Literal(value) => Some(value),
            Self::Secret(_) => None,
        }
    }

    #[must_use]
    pub const fn secret_name(&self) -> Option<&SecretName> {
        match self {
            Self::Secret(name) => Some(name),
            Self::Literal(_) => None,
        }
    }

    #[must_use]
    pub const fn is_secret(&self) -> bool {
        matches!(self, Self::Secret(_))
    }
}

impl<T: Serialize> Serialize for ConfigValue<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Literal(value) => value.serialize(serializer),
            Self::Secret(name) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("secretRef", name.as_str())?;
                map.end()
            }
        }
    }
}

impl<'de, T> Deserialize<'de> for ConfigValue<T>
where
    T: serde::de::DeserializeOwned,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml_ng::Value::deserialize(deserializer)?;
        if let serde_yaml_ng::Value::Mapping(map) = &value {
            let secret_key = serde_yaml_ng::Value::String("secretRef".to_owned());
            if let Some(secret_value) = map.get(&secret_key) {
                if map.len() != 1 {
                    return Err(D::Error::custom(
                        "a secretRef object must contain only the 'secretRef' key",
                    ));
                }
                let name = secret_value.as_str().ok_or_else(|| {
                    D::Error::custom("secretRef value must be a string secret name")
                })?;
                let secret = SecretName::new(name).map_err(D::Error::custom)?;
                return Ok(Self::Secret(secret));
            }
        }
        let literal = serde_yaml_ng::from_value(value).map_err(D::Error::custom)?;
        Ok(Self::Literal(literal))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{ConfigValue, SecretName};

    #[test]
    fn secret_name_grammar_is_enforced() {
        assert!(SecretName::is_valid("OPENAI_API_KEY"));
        assert!(SecretName::is_valid("A"));
        assert!(!SecretName::is_valid("lower"));
        assert!(!SecretName::is_valid("1LEAD"));
        assert!(!SecretName::is_valid("HAS-DASH"));
        assert!(!SecretName::is_valid(""));
    }

    #[test]
    fn config_value_round_trips_literal_and_secret() -> Result<(), Box<dyn Error + Send + Sync>> {
        let literal: ConfigValue<String> = serde_yaml_ng::from_str("https://api.example.test/v1")?;
        assert_eq!(
            literal,
            ConfigValue::Literal("https://api.example.test/v1".to_owned())
        );

        let secret: ConfigValue<String> = serde_yaml_ng::from_str("secretRef: OPENAI_API_KEY")?;
        assert_eq!(
            secret,
            ConfigValue::Secret(SecretName::new("OPENAI_API_KEY")?)
        );

        let emitted = serde_yaml_ng::to_string(&secret)?;
        assert_eq!(emitted, "secretRef: OPENAI_API_KEY\n");
        Ok(())
    }

    #[test]
    fn secret_ref_object_rejects_extra_keys() {
        let result: Result<ConfigValue<String>, _> =
            serde_yaml_ng::from_str("secretRef: NAME\nother: 1");
        assert!(result.is_err());
    }

    #[test]
    fn secret_ref_object_rejects_invalid_name() {
        let result: Result<ConfigValue<String>, _> = serde_yaml_ng::from_str("secretRef: lower");
        assert!(result.is_err());
    }
}
