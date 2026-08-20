use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{ProgramError, ProgramErrorKind};

pub const REFERENCE_KEY: &str = "$ref";

pub type StepEnvironment = BTreeMap<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepReference {
    pub step: String,
    #[serde(default)]
    pub pointer: String,
}

/// 遍历 JSON 模板中的全部引用。对象一旦含有 `$ref`，就必须严格匹配
/// `{ "$ref": { "step": "...", "pointer": "..." } }`，不允许与普通字段混用。
#[cfg(test)]
pub fn collect_references(template: &Value) -> Result<Vec<StepReference>, ProgramError> {
    let mut references = Vec::new();
    visit_template(template, "", &mut |reference, _| {
        references.push(reference.clone());
        Ok(())
    })?;
    Ok(references)
}

/// 递归解析模板。引用替换的是一个完整 JSON value，不做字符串插值或表达式计算。
pub fn resolve_template(
    template: &Value,
    environment: &StepEnvironment,
) -> Result<Value, ProgramError> {
    resolve_at(template, environment, "")
}

pub(crate) fn visit_references_at<F>(
    template: &Value,
    base_path: &str,
    visitor: &mut F,
) -> Result<(), ProgramError>
where
    F: FnMut(&StepReference, &str) -> Result<(), ProgramError>,
{
    visit_template(template, base_path, visitor)
}

fn resolve_at(
    value: &Value,
    environment: &StepEnvironment,
    path: &str,
) -> Result<Value, ProgramError> {
    if let Some(reference) = parse_reference(value, path)? {
        let envelope = environment.get(&reference.step).ok_or_else(|| {
            ProgramError::new(
                ProgramErrorKind::InvalidReference,
                format!("引用的步骤 '{}' 不存在或尚未完成", reference.step),
            )
            .at_path(path)
        })?;
        let selected = if reference.pointer.is_empty() {
            envelope
        } else {
            envelope.pointer(&reference.pointer).ok_or_else(|| {
                ProgramError::new(
                    ProgramErrorKind::InvalidReference,
                    format!(
                        "步骤 '{}' 的结果中不存在 JSON Pointer '{}'",
                        reference.step, reference.pointer
                    ),
                )
                .at_path(path)
            })?
        };
        return Ok(selected.clone());
    }

    match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                resolve_at(item, environment, &join_pointer(path, &index.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) => object
            .iter()
            .map(|(key, item)| {
                resolve_at(
                    item,
                    environment,
                    &join_pointer(path, &escape_pointer_token(key)),
                )
                .map(|resolved| (key.clone(), resolved))
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        scalar => Ok(scalar.clone()),
    }
}

fn visit_template<F>(value: &Value, path: &str, visitor: &mut F) -> Result<(), ProgramError>
where
    F: FnMut(&StepReference, &str) -> Result<(), ProgramError>,
{
    if let Some(reference) = parse_reference(value, path)? {
        return visitor(&reference, path);
    }

    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                visit_template(item, &join_pointer(path, &index.to_string()), visitor)?;
            }
        }
        Value::Object(object) => {
            for (key, item) in object {
                visit_template(
                    item,
                    &join_pointer(path, &escape_pointer_token(key)),
                    visitor,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_reference(value: &Value, path: &str) -> Result<Option<StepReference>, ProgramError> {
    let Value::Object(object) = value else {
        return Ok(None);
    };
    let Some(reference_value) = object.get(REFERENCE_KEY) else {
        return Ok(None);
    };
    if object.len() != 1 {
        return Err(invalid_reference(path, "引用对象只能包含 '$ref' 一个字段"));
    }

    let reference: StepReference =
        serde_json::from_value(reference_value.clone()).map_err(|e| {
            invalid_reference(path, format!("'$ref' 必须包含 step 与可选 pointer：{e}"))
        })?;
    if reference.step.is_empty() {
        return Err(invalid_reference(path, "引用的 step 不能为空"));
    }
    validate_json_pointer(&reference.pointer)
        .map_err(|message| invalid_reference(path, message))?;
    Ok(Some(reference))
}

fn validate_json_pointer(pointer: &str) -> Result<(), &'static str> {
    if pointer.is_empty() {
        return Ok(());
    }
    if !pointer.starts_with('/') {
        return Err("pointer 必须为空或以 '/' 开头");
    }

    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            let Some(escaped) = bytes.get(index + 1) else {
                return Err("pointer 中 '~' 必须使用 '~0' 或 '~1' 转义");
            };
            if !matches!(escaped, b'0' | b'1') {
                return Err("pointer 中 '~' 必须使用 '~0' 或 '~1' 转义");
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn invalid_reference(path: &str, message: impl Into<String>) -> ProgramError {
    ProgramError::new(ProgramErrorKind::InvalidReference, message).at_path(path)
}

fn join_pointer(base: &str, token: &str) -> String {
    format!("{base}/{token}")
}

fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::error::ProgramErrorKind;
    use super::{collect_references, resolve_template, StepEnvironment};

    #[test]
    fn collects_nested_references_without_interpreting_strings() {
        let template = json!({
            "literal": "$ref is only special as an object key",
            "items": [
                { "$ref": { "step": "search", "pointer": "/data/files" } },
                { "$ref": { "step": "read" } }
            ]
        });

        let references = collect_references(&template).expect("collect references");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].step, "search");
        assert_eq!(references[0].pointer, "/data/files");
        assert_eq!(references[1].step, "read");
        assert_eq!(references[1].pointer, "");
    }

    #[test]
    fn resolves_whole_values_and_rfc6901_escaped_pointers() {
        let mut environment = StepEnvironment::new();
        environment.insert(
            "search".to_string(),
            json!({ "data": { "a/b": { "~key": ["src/lib.rs"] } } }),
        );
        let template = json!({
            "path": {
                "$ref": {
                    "step": "search",
                    "pointer": "/data/a~1b/~0key/0"
                }
            }
        });

        let resolved = resolve_template(&template, &environment).expect("resolve template");
        assert_eq!(resolved, json!({ "path": "src/lib.rs" }));
    }

    #[test]
    fn rejects_malformed_or_ambiguous_reference_objects() {
        let mixed = json!({
            "$ref": { "step": "read" },
            "fallback": "not allowed"
        });
        let error = collect_references(&mixed).unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::InvalidReference);

        let extra_inner_field = json!({
            "$ref": { "step": "read", "expression": "evil()" }
        });
        assert_eq!(
            collect_references(&extra_inner_field).unwrap_err().kind,
            ProgramErrorKind::InvalidReference
        );

        let invalid_pointer = json!({
            "$ref": { "step": "read", "pointer": "/bad~2escape" }
        });
        assert_eq!(
            collect_references(&invalid_pointer).unwrap_err().kind,
            ProgramErrorKind::InvalidReference
        );
    }

    #[test]
    fn reports_missing_step_and_missing_pointer() {
        let missing_step = json!({ "$ref": { "step": "unknown" } });
        let error = resolve_template(&missing_step, &StepEnvironment::new()).unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::InvalidReference);

        let mut environment = StepEnvironment::new();
        environment.insert("read".to_string(), json!({ "output": "ok" }));
        let missing_pointer = json!({
            "$ref": { "step": "read", "pointer": "/data" }
        });
        let error = resolve_template(&missing_pointer, &environment).unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::InvalidReference);
    }
}
