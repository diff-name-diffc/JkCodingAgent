//! `validate.rs` 的单元测试（迁移自旧 `agent/tools/program/validate.rs` 内联 tests）。

use std::collections::BTreeMap;

use serde_json::json;

use super::super::error::ProgramErrorKind;
use super::{
    parse_and_validate_program, validate_program_value, CapabilityCatalog, CapabilityPolicy,
    ProgramLimits,
};

struct Catalog(BTreeMap<&'static str, CapabilityPolicy>);

impl Catalog {
    fn standard() -> Self {
        Self(BTreeMap::from([
            ("read_file", CapabilityPolicy::parallel_readonly()),
            ("grep", CapabilityPolicy::parallel_readonly()),
            ("write_file", CapabilityPolicy::sequential()),
        ]))
    }
}

impl CapabilityCatalog for Catalog {
    fn capability(&self, tool_name: &str) -> Option<CapabilityPolicy> {
        self.0.get(tool_name).copied()
    }
}

fn valid_program() -> serde_json::Value {
    json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "call",
                    "id": "search",
                    "tool": "grep",
                    "arguments": { "pattern": "ToolRuntime" }
                },
                {
                    "op": "parallel",
                    "branches": [
                        {
                            "op": "call",
                            "id": "left",
                            "tool": "read_file",
                            "arguments": {
                                "path": { "$ref": { "step": "search", "pointer": "/output" } }
                            }
                        },
                        {
                            "op": "sequence",
                            "steps": [
                                {
                                    "op": "call",
                                    "id": "right",
                                    "tool": "read_file",
                                    "arguments": { "path": "src/lib.rs" }
                                }
                            ]
                        }
                    ]
                },
                {
                    "op": "return",
                    "value": {
                        "left": { "$ref": { "step": "left", "pointer": "/output" } },
                        "right": { "$ref": { "step": "right", "pointer": "/output" } }
                    }
                }
            ]
        }
    })
}

#[test]
fn accepts_valid_program() {
    validate_program_value(
        &valid_program(),
        &Catalog::standard(),
        &ProgramLimits::default(),
    )
    .expect("valid program");
}

#[test]
fn rejects_wrong_version_root_and_return_placement() {
    let mut wrong_version = valid_program();
    wrong_version["version"] = json!(2);
    assert_eq!(
        validate_program_value(
            &wrong_version,
            &Catalog::standard(),
            &ProgramLimits::default()
        )
        .unwrap_err()
        .kind,
        ProgramErrorKind::Validation
    );

    let non_sequence_root = json!({
        "version": 1,
        "root": { "op": "return", "value": null }
    });
    assert_eq!(
        validate_program_value(
            &non_sequence_root,
            &Catalog::standard(),
            &ProgramLimits::default()
        )
        .unwrap_err()
        .kind,
        ProgramErrorKind::Validation
    );

    let nested_return = json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "sequence",
                    "steps": [{ "op": "return", "value": null }]
                },
                { "op": "return", "value": null }
            ]
        }
    });
    assert_eq!(
        validate_program_value(
            &nested_return,
            &Catalog::standard(),
            &ProgramLimits::default()
        )
        .unwrap_err()
        .kind,
        ProgramErrorKind::Validation
    );
}

#[test]
fn enforces_definite_before_use_in_sequence() {
    let forward_reference = json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "call",
                    "id": "first",
                    "tool": "read_file",
                    "arguments": {
                        "path": { "$ref": { "step": "later", "pointer": "/output" } }
                    }
                },
                {
                    "op": "call",
                    "id": "later",
                    "tool": "read_file",
                    "arguments": { "path": "a.rs" }
                },
                { "op": "return", "value": null }
            ]
        }
    });

    let error = validate_program_value(
        &forward_reference,
        &Catalog::standard(),
        &ProgramLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ProgramErrorKind::InvalidReference);
}

#[test]
fn parallel_branches_cannot_reference_siblings() {
    let sibling_reference = json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "parallel",
                    "branches": [
                        {
                            "op": "call",
                            "id": "left",
                            "tool": "read_file",
                            "arguments": { "path": "a.rs" }
                        },
                        {
                            "op": "call",
                            "id": "right",
                            "tool": "read_file",
                            "arguments": {
                                "path": { "$ref": { "step": "left", "pointer": "/output" } }
                            }
                        }
                    ]
                },
                { "op": "return", "value": null }
            ]
        }
    });

    let error = validate_program_value(
        &sibling_reference,
        &Catalog::standard(),
        &ProgramLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ProgramErrorKind::InvalidReference);
}

#[test]
fn parallel_outputs_are_available_after_join() {
    let validated = validate_program_value(
        &valid_program(),
        &Catalog::standard(),
        &ProgramLimits::default(),
    );
    assert!(validated.is_ok());
}

#[test]
fn rejects_duplicate_or_invalid_step_ids() {
    for (first_id, second_id) in [("same", "same"), ("1bad", "valid")] {
        let program = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "call", "id": first_id, "tool": "read_file",
                        "arguments": { "path": "a.rs" }
                    },
                    {
                        "op": "call", "id": second_id, "tool": "read_file",
                        "arguments": { "path": "b.rs" }
                    },
                    { "op": "return", "value": null }
                ]
            }
        });
        assert_eq!(
            validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
                .unwrap_err()
                .kind,
            ProgramErrorKind::Validation
        );
    }
}

#[test]
fn rejects_unknown_control_plane_and_recursive_tools() {
    for tool in ["unknown", "message", "run_tool_program", "call_sub_agent"] {
        let program = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    { "op": "call", "id": "step", "tool": tool, "arguments": {} },
                    { "op": "return", "value": null }
                ]
            }
        });
        assert_eq!(
            validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
                .unwrap_err()
                .kind,
            ProgramErrorKind::PolicyDenied
        );
    }
}

#[test]
fn rejects_non_parallel_capability_anywhere_in_parallel_subtree() {
    let program = json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "parallel",
                    "branches": [
                        {
                            "op": "call", "id": "read", "tool": "read_file",
                            "arguments": { "path": "a.rs" }
                        },
                        {
                            "op": "sequence",
                            "steps": [{
                                "op": "call", "id": "write", "tool": "write_file",
                                "arguments": { "path": "b.rs", "content": "x" }
                            }]
                        }
                    ]
                },
                { "op": "return", "value": null }
            ]
        }
    });

    let error =
        validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
            .unwrap_err();
    assert_eq!(error.kind, ProgramErrorKind::PolicyDenied);
    assert_eq!(error.tool.as_deref(), Some("write_file"));
}

#[test]
fn enforces_node_call_depth_parallel_and_input_budgets() {
    let catalog = Catalog::standard();

    let limits = ProgramLimits {
        max_nodes: 2,
        ..ProgramLimits::default()
    };
    assert_eq!(
        validate_program_value(&valid_program(), &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::LimitExceeded
    );

    let limits = ProgramLimits {
        max_calls: 2,
        ..ProgramLimits::default()
    };
    assert_eq!(
        validate_program_value(&valid_program(), &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::LimitExceeded
    );

    let limits = ProgramLimits {
        max_depth: 3,
        ..ProgramLimits::default()
    };
    assert_eq!(
        validate_program_value(&valid_program(), &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::LimitExceeded
    );

    let limits = ProgramLimits {
        max_parallel_branches: 2,
        ..ProgramLimits::default()
    };
    let three_branches = json!({
        "version": 1,
        "root": { "op": "sequence", "steps": [
            { "op": "parallel", "branches": [
                { "op": "call", "id": "a", "tool": "read_file", "arguments": {} },
                { "op": "call", "id": "b", "tool": "read_file", "arguments": {} },
                { "op": "call", "id": "c", "tool": "read_file", "arguments": {} }
            ] },
            { "op": "return", "value": null }
        ] }
    });
    assert_eq!(
        validate_program_value(&three_branches, &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::LimitExceeded
    );

    let limits = ProgramLimits {
        max_input_bytes: 8,
        ..ProgramLimits::default()
    };
    assert_eq!(
        parse_and_validate_program(b"{}", &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::Parse
    );
    assert_eq!(
        parse_and_validate_program(b"{\"123456789\":true}", &catalog, &limits)
            .unwrap_err()
            .kind,
        ProgramErrorKind::LimitExceeded
    );
}

#[test]
fn rejects_malformed_reference_during_static_validation() {
    let malformed = json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "call", "id": "read", "tool": "read_file",
                    "arguments": {
                        "path": {
                            "$ref": { "step": "prior" },
                            "fallback": "a.rs"
                        }
                    }
                },
                { "op": "return", "value": null }
            ]
        }
    });
    assert_eq!(
        validate_program_value(&malformed, &Catalog::standard(), &ProgramLimits::default())
            .unwrap_err()
            .kind,
        ProgramErrorKind::InvalidReference
    );
}

#[test]
fn closure_can_supply_capability_catalog() {
    let catalog =
        |name: &str| (name == "read_file").then_some(CapabilityPolicy::parallel_readonly());
    let program = json!({
        "version": 1,
        "root": { "op": "sequence", "steps": [
            { "op": "call", "id": "read", "tool": "read_file", "arguments": {} },
            { "op": "return", "value": null }
        ] }
    });
    assert!(validate_program_value(&program, &catalog, &ProgramLimits::default()).is_ok());
}
