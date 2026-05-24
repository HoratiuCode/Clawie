use crate::json::{JsonError, JsonValue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredOutputSpec {
    pub required_keys: Vec<String>,
    pub allow_array: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredOutput {
    pub value: JsonValue,
}

impl StructuredOutputSpec {
    #[must_use]
    pub fn object(required_keys: Vec<String>) -> Self {
        Self {
            required_keys,
            allow_array: false,
        }
    }

    pub fn parse(&self, source: &str) -> Result<StructuredOutput, JsonError> {
        let value = JsonValue::parse(source)?;
        match &value {
            JsonValue::Object(object) => {
                for key in &self.required_keys {
                    if !object.contains_key(key) {
                        return Err(JsonError::new(format!(
                            "structured output is missing required key `{key}`"
                        )));
                    }
                }
            }
            JsonValue::Array(_) if self.allow_array => {}
            _ => {
                return Err(JsonError::new(
                    "structured output must be a JSON object".to_string(),
                ))
            }
        }
        Ok(StructuredOutput { value })
    }
}

#[must_use]
pub fn extract_json_candidate(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return Some(trimmed);
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (end > start).then_some(&trimmed[start..=end])
}

#[cfg(test)]
mod tests {
    use super::{extract_json_candidate, StructuredOutputSpec};

    #[test]
    fn validates_required_object_keys() {
        let spec = StructuredOutputSpec::object(vec!["summary".to_string()]);

        assert!(spec.parse(r#"{"summary":"ok"}"#).is_ok());
        assert!(spec.parse(r#"{"other":"no"}"#).is_err());
    }

    #[test]
    fn extracts_json_from_surrounding_text() {
        assert_eq!(
            extract_json_candidate("result:\n{\"ok\":true}\n"),
            Some(r#"{"ok":true}"#)
        );
    }
}
