use crate::llm::StructuredOutputSpec;

pub fn parse_structured_output(
    spec: &StructuredOutputSpec,
    text: &str,
) -> anyhow::Result<serde_json::Value> {
    spec.validate()?;

    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("structured_output_empty_response");
    }

    let parsed = serde_json::from_str(trimmed)
        .map_err(|e| anyhow::anyhow!("structured_output_invalid_json: {e}"))?;
    validate_value_against_schema(&spec.schema, &parsed, "$")
        .map_err(|e| anyhow::anyhow!("structured_output_schema_validation_failed: {e}"))?;
    Ok(parsed)
}

fn validate_value_against_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    let Some(schema_obj) = schema.as_object() else {
        return Ok(());
    };

    if let Some(allowed) = schema_obj.get("enum").and_then(|v| v.as_array()) {
        if !allowed.iter().any(|candidate| candidate == value) {
            return Err(format!("{path}: value not in enum"));
        }
    }

    if let Some(type_spec) = schema_obj.get("type") {
        if !matches_type_spec(value, type_spec) {
            return Err(format!(
                "{path}: expected {}",
                describe_type_spec(type_spec)
            ));
        }
    }

    let has_object_keywords = schema_obj.contains_key("required")
        || schema_obj.contains_key("properties")
        || schema_obj.contains_key("additionalProperties");
    if has_object_keywords {
        let object_value = value
            .as_object()
            .ok_or_else(|| format!("{path}: expected object"))?;
        if let Some(required) = schema_obj.get("required").and_then(|v| v.as_array()) {
            for entry in required {
                let key = entry
                    .as_str()
                    .ok_or_else(|| format!("{path}: schema required entries must be strings"))?;
                if !object_value.contains_key(key) {
                    return Err(format!("{path}: missing required field `{key}`"));
                }
            }
        }

        let properties = schema_obj.get("properties").and_then(|v| v.as_object());
        if let Some(properties) = properties {
            for (key, property_schema) in properties {
                if let Some(property_value) = object_value.get(key) {
                    validate_value_against_schema(
                        property_schema,
                        property_value,
                        &format!("{path}.{key}"),
                    )?;
                }
            }

            if schema_obj
                .get("additionalProperties")
                .and_then(|v| v.as_bool())
                == Some(false)
            {
                for key in object_value.keys() {
                    if !properties.contains_key(key) {
                        return Err(format!("{path}: unexpected property `{key}`"));
                    }
                }
            }
        } else if schema_obj
            .get("additionalProperties")
            .and_then(|v| v.as_bool())
            == Some(false)
            && !object_value.is_empty()
        {
            let key = object_value.keys().next().cloned().unwrap_or_default();
            return Err(format!("{path}: unexpected property `{key}`"));
        }
    }

    if let Some(item_schema) = schema_obj.get("items") {
        let arr = value
            .as_array()
            .ok_or_else(|| format!("{path}: expected array"))?;
        for (idx, item) in arr.iter().enumerate() {
            validate_value_against_schema(item_schema, item, &format!("{path}[{idx}]"))?;
        }
    }

    Ok(())
}

fn matches_type_spec(value: &serde_json::Value, type_spec: &serde_json::Value) -> bool {
    match type_spec {
        serde_json::Value::String(t) => matches_type(value, t),
        serde_json::Value::Array(types) => types
            .iter()
            .filter_map(|v| v.as_str())
            .any(|t| matches_type(value, t)),
        _ => true,
    }
}

fn describe_type_spec(type_spec: &serde_json::Value) -> String {
    match type_spec {
        serde_json::Value::String(t) => t.to_string(),
        serde_json::Value::Array(types) => {
            let names = types
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("|");
            if names.is_empty() {
                "unknown".to_string()
            } else {
                names
            }
        }
        _ => "unknown".to_string(),
    }
}

fn matches_type(value: &serde_json::Value, typ: &str) -> bool {
    match typ {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_structured_output;
    use crate::llm::StructuredOutputSpec;

    fn spec(schema: serde_json::Value) -> StructuredOutputSpec {
        StructuredOutputSpec {
            name: "final_answer".to_string(),
            schema,
            description: None,
            strict: true,
        }
    }

    #[test]
    fn structured_output_accepts_value_that_matches_schema() {
        let out = parse_structured_output(
            &spec(serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                },
                "required": ["summary"],
                "additionalProperties": false
            })),
            r#"{"summary":"done"}"#,
        )
        .unwrap();
        assert_eq!(out, serde_json::json!({"summary":"done"}));
    }

    #[test]
    fn structured_output_rejects_missing_required_fields() {
        let err = parse_structured_output(
            &spec(serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                },
                "required": ["summary"],
                "additionalProperties": false
            })),
            r#"{"other":"x"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing required field `summary`"));
    }

    #[test]
    fn structured_output_rejects_additional_properties_when_disallowed() {
        let err = parse_structured_output(
            &spec(serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                },
                "required": ["summary"],
                "additionalProperties": false
            })),
            r#"{"summary":"ok","extra":1}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unexpected property `extra`"));
    }
}
