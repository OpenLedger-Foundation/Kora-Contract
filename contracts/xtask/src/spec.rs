//! Contract spec/ABI extraction for automated code generation (issue #654).
//! This module provides functionality to extract contract function signatures and
//! specifications (XDR) into a machine-readable JSON format usable by both TypeScript
//! codegen and documentation generation.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractSpec {
    pub name: String,
    pub version: String,
    pub functions: Vec<FunctionSpec>,
    pub types: Vec<TypeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionSpec {
    pub name: String,
    pub is_readonly: bool,
    pub parameters: Vec<ParameterSpec>,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterSpec {
    pub name: String,
    pub param_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeSpec {
    pub name: String,
    pub fields: Vec<TypeField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeField {
    pub name: String,
    pub field_type: String,
}

/// Extract contract specification from XDR representation.
pub fn extract_contract_spec(xdr_path: &Path) -> Result<ContractSpec, Box<dyn std::error::Error>> {
    let _xdr_content = fs::read_to_string(xdr_path)?;
    // Placeholder for actual XDR parsing
    Ok(ContractSpec {
        name: "placeholder".to_string(),
        version: "0.1.0".to_string(),
        functions: vec![],
        types: vec![],
    })
}

/// Validate that extracted spec matches expected structure.
pub fn validate_spec(spec: &ContractSpec) -> Result<(), String> {
    if spec.name.is_empty() {
        return Err("Contract name must not be empty".to_string());
    }
    if spec.version.is_empty() {
        return Err("Contract version must not be empty".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_spec_serialization() {
        let spec = ContractSpec {
            name: "marketplace".to_string(),
            version: "1.0.0".to_string(),
            functions: vec![
                FunctionSpec {
                    name: "list_invoice".to_string(),
                    is_readonly: true,
                    parameters: vec![
                        ParameterSpec {
                            name: "id".to_string(),
                            param_type: "u64".to_string(),
                        },
                    ],
                    return_type: Some("Invoice".to_string()),
                },
                FunctionSpec {
                    name: "create_listing".to_string(),
                    is_readonly: false,
                    parameters: vec![
                        ParameterSpec {
                            name: "invoice_id".to_string(),
                            param_type: "u64".to_string(),
                        },
                        ParameterSpec {
                            name: "price".to_string(),
                            param_type: "i128".to_string(),
                        },
                    ],
                    return_type: None,
                },
            ],
            types: vec![
                TypeSpec {
                    name: "Invoice".to_string(),
                    fields: vec![
                        TypeField {
                            name: "id".to_string(),
                            field_type: "u64".to_string(),
                        },
                        TypeField {
                            name: "amount".to_string(),
                            field_type: "i128".to_string(),
                        },
                    ],
                },
            ],
        };

        let json = serde_json::to_string(&spec).expect("serialization failed");
        let parsed: ContractSpec = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(parsed.name, "marketplace");
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.functions.len(), 2);
        assert_eq!(parsed.types.len(), 1);
    }

    #[test]
    fn test_function_spec_readonly_flag() {
        let readonly_func = FunctionSpec {
            name: "balance_of".to_string(),
            is_readonly: true,
            parameters: vec![],
            return_type: Some("i128".to_string()),
        };

        let write_func = FunctionSpec {
            name: "transfer".to_string(),
            is_readonly: false,
            parameters: vec![
                ParameterSpec {
                    name: "to".to_string(),
                    param_type: "Address".to_string(),
                },
                ParameterSpec {
                    name: "amount".to_string(),
                    param_type: "i128".to_string(),
                },
            ],
            return_type: None,
        };

        assert!(readonly_func.is_readonly);
        assert!(!write_func.is_readonly);
    }

    #[test]
    fn test_validate_spec_requires_name() {
        let spec = ContractSpec {
            name: "".to_string(),
            version: "1.0.0".to_string(),
            functions: vec![],
            types: vec![],
        };

        let result = validate_spec(&spec);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Contract name must not be empty");
    }

    #[test]
    fn test_validate_spec_requires_version() {
        let spec = ContractSpec {
            name: "test_contract".to_string(),
            version: "".to_string(),
            functions: vec![],
            types: vec![],
        };

        let result = validate_spec(&spec);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Contract version must not be empty");
    }

    #[test]
    fn test_validate_spec_accepts_valid_spec() {
        let spec = ContractSpec {
            name: "valid_contract".to_string(),
            version: "2.0.0".to_string(),
            functions: vec![],
            types: vec![],
        };

        let result = validate_spec(&spec);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parameter_spec_with_multiple_types() {
        let params = vec![
            ParameterSpec {
                name: "admin".to_string(),
                param_type: "Address".to_string(),
            },
            ParameterSpec {
                name: "threshold".to_string(),
                param_type: "u32".to_string(),
            },
            ParameterSpec {
                name: "data".to_string(),
                param_type: "Bytes".to_string(),
            },
        ];

        assert_eq!(params.len(), 3);
        assert!(params.iter().all(|p| !p.name.is_empty() && !p.param_type.is_empty()));
    }

    #[test]
    fn test_type_spec_with_complex_fields() {
        let type_spec = TypeSpec {
            name: "ProposalData".to_string(),
            fields: vec![
                TypeField {
                    name: "id".to_string(),
                    field_type: "u64".to_string(),
                },
                TypeField {
                    name: "proposer".to_string(),
                    field_type: "Address".to_string(),
                },
                TypeField {
                    name: "votes".to_string(),
                    field_type: "Map<Address, bool>".to_string(),
                },
            ],
        };

        assert_eq!(type_spec.name, "ProposalData");
        assert_eq!(type_spec.fields.len(), 3);
    }

    #[test]
    fn test_spec_roundtrip_through_json() {
        let original = ContractSpec {
            name: "treasury".to_string(),
            version: "3.1.0".to_string(),
            functions: vec![
                FunctionSpec {
                    name: "collect_fee".to_string(),
                    is_readonly: false,
                    parameters: vec![
                        ParameterSpec {
                            name: "amount".to_string(),
                            param_type: "i128".to_string(),
                        },
                    ],
                    return_type: None,
                },
            ],
            types: vec![],
        };

        let json = serde_json::to_string_pretty(&original).expect("serialization failed");
        let restored: ContractSpec = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(original, restored);
    }
}
