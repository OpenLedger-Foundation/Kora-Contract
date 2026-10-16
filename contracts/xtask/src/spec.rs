//! Contract spec/ABI extraction for automated code generation (issue #654, #937).
//! This module provides functionality to extract contract function signatures and
//! specifications (XDR) into machine-readable JSON and interface txt formats usable
//! by both TypeScript SDK codegen and interface-compat checks.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use soroban_spec::read::from_wasm;
use stellar_xdr::curr::{Limits, ReadXdr, ScSpecEntry, ScSpecTypeDef};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractSpec {
    pub name: String,
    pub version: String,
    pub functions: Vec<FunctionSpec>,
    pub types: Vec<TypeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionSpec {
    pub name: String,
    pub is_readonly: bool,
    pub parameters: Vec<ParameterSpec>,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterSpec {
    pub name: String,
    pub param_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeSpec {
    pub name: String,
    pub fields: Vec<TypeField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeField {
    pub name: String,
    pub field_type: String,
}

impl ContractSpec {
    /// Serialize this specification to pretty-printed JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Format this specification as interface lines suitable for `check-interface-compat.sh`.
    /// Each function is emitted as: `fn <name>(<param>: <type>, ...) -> <return_type>`
    /// Lines are sorted alphabetically for deterministic diffs.
    pub fn to_interface_txt(&self) -> String {
        let mut lines = Vec::new();
        for f in &self.functions {
            let params = f
                .parameters
                .iter()
                .map(|p| format!("{}: {}", p.name, p.param_type))
                .collect::<Vec<_>>()
                .join(", ");
            let ret = f.return_type.as_deref().unwrap_or("()");
            lines.push(format!("fn {}({}) -> {}", f.name, params, ret));
        }
        lines.sort();
        lines.join("\n")
    }

    /// Validate that the extracted spec satisfies structural requirements.
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<(), String> {
        validate_spec(self)
    }
}

/// Determine whether a function is read-only by convention.
pub fn is_readonly_fn_name(name: &str) -> bool {
    name.starts_with("get_")
        || name.starts_with("is_")
        || name.starts_with("has_")
        || name.starts_with("check_")
        || name.starts_with("query_")
        || name.starts_with("read_")
        || name.starts_with("total_")
        || name.starts_with("balance_")
        || name == "balance_of"
        || name == "decimals"
        || name == "name"
        || name == "symbol"
        || name == "version"
}

/// Convert an XDR `ScSpecTypeDef` into a canonical, human-readable type string.
/// Never panics on unknown or future SCVal type variants.
pub fn format_type_def(def: &ScSpecTypeDef) -> String {
    match def {
        ScSpecTypeDef::Val => "Val".to_string(),
        ScSpecTypeDef::U64 => "u64".to_string(),
        ScSpecTypeDef::I64 => "i64".to_string(),
        ScSpecTypeDef::U128 => "u128".to_string(),
        ScSpecTypeDef::I128 => "i128".to_string(),
        ScSpecTypeDef::U32 => "u32".to_string(),
        ScSpecTypeDef::I32 => "i32".to_string(),
        ScSpecTypeDef::U256 => "u256".to_string(),
        ScSpecTypeDef::I256 => "i256".to_string(),
        ScSpecTypeDef::Bool => "bool".to_string(),
        ScSpecTypeDef::Symbol => "Symbol".to_string(),
        ScSpecTypeDef::String => "String".to_string(),
        ScSpecTypeDef::Bytes => "Bytes".to_string(),
        ScSpecTypeDef::Address => "Address".to_string(),
        ScSpecTypeDef::Timepoint => "Timepoint".to_string(),
        ScSpecTypeDef::Duration => "Duration".to_string(),
        ScSpecTypeDef::Void => "()".to_string(),
        ScSpecTypeDef::Option(opt) => format!("Option<{}>", format_type_def(&opt.value_type)),
        ScSpecTypeDef::Result(res) => format!(
            "Result<{}, {}>",
            format_type_def(&res.ok_type),
            format_type_def(&res.error_type)
        ),
        ScSpecTypeDef::Vec(v) => format!("Vec<{}>", format_type_def(&v.element_type)),
        ScSpecTypeDef::Map(m) => format!(
            "Map<{}, {}>",
            format_type_def(&m.key_type),
            format_type_def(&m.value_type)
        ),
        ScSpecTypeDef::Tuple(t) => {
            let inner: Vec<String> = t.value_types.iter().map(format_type_def).collect();
            format!("({})", inner.join(", "))
        }
        ScSpecTypeDef::BytesN(b) => format!("BytesN<{}>", b.n),
        ScSpecTypeDef::Udt(udt) => udt.name.to_utf8_string_lossy(),
        _ => "Unknown".to_string(),
    }
}

/// Transform raw XDR `ScSpecEntry` items into structured `ContractSpec`.
pub fn spec_entries_to_contract_spec(
    name: &str,
    version: &str,
    entries: &[ScSpecEntry],
) -> ContractSpec {
    let mut functions = Vec::new();
    let mut types = Vec::new();

    for entry in entries {
        match entry {
            ScSpecEntry::FunctionV0(f) => {
                let fn_name = f.name.to_utf8_string_lossy();
                let parameters: Vec<ParameterSpec> = f
                    .inputs
                    .iter()
                    .map(|input| ParameterSpec {
                        name: input.name.to_utf8_string_lossy(),
                        param_type: format_type_def(&input.type_),
                    })
                    .collect();

                let return_type = if f.outputs.is_empty() {
                    None
                } else if f.outputs.len() == 1 {
                    let formatted = format_type_def(&f.outputs[0]);
                    if formatted == "()" {
                        None
                    } else {
                        Some(formatted)
                    }
                } else {
                    let types_str: Vec<String> = f.outputs.iter().map(format_type_def).collect();
                    Some(format!("({})", types_str.join(", ")))
                };

                let is_readonly = is_readonly_fn_name(&fn_name);
                functions.push(FunctionSpec {
                    name: fn_name,
                    is_readonly,
                    parameters,
                    return_type,
                });
            }
            ScSpecEntry::UdtStructV0(s) => {
                let struct_name = s.name.to_utf8_string_lossy();
                let fields: Vec<TypeField> = s
                    .fields
                    .iter()
                    .map(|field| TypeField {
                        name: field.name.to_utf8_string_lossy(),
                        field_type: format_type_def(&field.type_),
                    })
                    .collect();
                types.push(TypeSpec {
                    name: struct_name,
                    fields,
                });
            }
            ScSpecEntry::UdtUnionV0(u) => {
                let union_name = u.name.to_utf8_string_lossy();
                let fields: Vec<TypeField> = u
                    .cases
                    .iter()
                    .map(|case| match case {
                        stellar_xdr::curr::ScSpecUdtUnionCaseV0::VoidV0(v) => TypeField {
                            name: v.name.to_utf8_string_lossy(),
                            field_type: "()".to_string(),
                        },
                        stellar_xdr::curr::ScSpecUdtUnionCaseV0::TupleV0(t) => {
                            let inner: Vec<String> = t.type_.iter().map(format_type_def).collect();
                            TypeField {
                                name: t.name.to_utf8_string_lossy(),
                                field_type: format!("({})", inner.join(", ")),
                            }
                        }
                    })
                    .collect();
                types.push(TypeSpec {
                    name: union_name,
                    fields,
                });
            }
            ScSpecEntry::UdtEnumV0(e) => {
                let enum_name = e.name.to_utf8_string_lossy();
                let fields: Vec<TypeField> = e
                    .cases
                    .iter()
                    .map(|case| TypeField {
                        name: case.name.to_utf8_string_lossy(),
                        field_type: format!("u32 = {}", case.value),
                    })
                    .collect();
                types.push(TypeSpec {
                    name: enum_name,
                    fields,
                });
            }
            ScSpecEntry::UdtErrorEnumV0(err_e) => {
                let enum_name = err_e.name.to_utf8_string_lossy();
                let fields: Vec<TypeField> = err_e
                    .cases
                    .iter()
                    .map(|case| TypeField {
                        name: case.name.to_utf8_string_lossy(),
                        field_type: format!("u32 = {}", case.value),
                    })
                    .collect();
                types.push(TypeSpec {
                    name: enum_name,
                    fields,
                });
            }
        }
    }

    ContractSpec {
        name: name.to_string(),
        version: version.to_string(),
        functions,
        types,
    }
}

/// Extract contract specification directly from raw WASM binary bytes.
pub fn extract_contract_spec_from_wasm(
    wasm_bytes: &[u8],
    name: &str,
    version: &str,
) -> Result<ContractSpec, Box<dyn std::error::Error>> {
    let entries = from_wasm(wasm_bytes)?;
    Ok(spec_entries_to_contract_spec(name, version, &entries))
}

/// Extract contract specification from a file path (.wasm, .xdr, or .json).
pub fn extract_contract_spec(path: &Path) -> Result<ContractSpec, Box<dyn std::error::Error>> {
    let raw = fs::read(path)?;
    let contract_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("contract")
        .to_string();

    // Check if WASM binary magic \0asm (0x00, 0x61, 0x73, 0x6d)
    if raw.starts_with(b"\0asm") {
        return extract_contract_spec_from_wasm(&raw, &contract_name, "1.0.0");
    }

    // Check if JSON file
    if let Ok(text) = std::str::from_utf8(&raw) {
        if let Ok(spec) = serde_json::from_str::<ContractSpec>(text) {
            return Ok(spec);
        }
    }

    // Attempt parsing as raw XDR stream
    let mut entries = Vec::new();
    let mut cursor = raw.as_slice();
    let mut limited = stellar_xdr::curr::Limited::new(&mut cursor, Limits::none());
    while let Ok(entry) = ScSpecEntry::read_xdr(&mut limited) {
        entries.push(entry);
    }

    if !entries.is_empty() {
        return Ok(spec_entries_to_contract_spec(
            &contract_name,
            "1.0.0",
            &entries,
        ));
    }

    Err(format!(
        "Unable to parse contract spec from {} (not valid WASM, XDR stream, or JSON)",
        path.display()
    )
    .into())
}

/// Validate that extracted spec matches expected structure.
#[allow(dead_code)]
pub fn validate_spec(spec: &ContractSpec) -> Result<(), String> {
    if spec.name.is_empty() {
        return Err("Contract name must not be empty".to_string());
    }
    if spec.version.is_empty() {
        return Err("Contract version must not be empty".to_string());
    }
    Ok(())
}

/// Helper to synthesize a minimal valid WASM binary containing a `contractspecv0` custom section.
#[cfg(test)]
pub fn build_synthetic_wasm_with_spec(entries: &[ScSpecEntry]) -> Vec<u8> {
    use stellar_xdr::curr::WriteXdr;
    let mut xdr_payload = Vec::new();
    for entry in entries {
        let entry_bytes = entry
            .to_xdr(Limits::none())
            .expect("XDR serialization failed");
        xdr_payload.extend_from_slice(&entry_bytes);
    }

    let mut wasm = Vec::new();
    // WASM magic \0asm + version 1
    wasm.extend_from_slice(b"\0asm\x01\x00\x00\x00");

    // Custom section id: 0
    let section_name = b"contractspecv0";
    let mut section_content = Vec::new();
    // LEB128 name length
    section_content.push(section_name.len() as u8);
    section_content.extend_from_slice(section_name);
    section_content.extend_from_slice(&xdr_payload);

    wasm.push(0x00); // Section ID 0 (Custom)
                     // LEB128 section length (supports up to 16KB in 2 bytes for tests)
    let len = section_content.len();
    if len < 128 {
        wasm.push(len as u8);
    } else {
        wasm.push(((len & 0x7F) | 0x80) as u8);
        wasm.push((len >> 7) as u8);
    }
    wasm.extend_from_slice(&section_content);

    wasm
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecTypeBytesN, ScSpecTypeMap, ScSpecTypeOption,
        ScSpecTypeResult, ScSpecTypeTuple, ScSpecTypeUdt, ScSpecTypeVec, ScSpecUdtStructFieldV0,
        ScSpecUdtStructV0, ScSymbol, StringM, VecM,
    };

    #[test]
    fn test_contract_spec_serialization() {
        let spec = ContractSpec {
            name: "marketplace".to_string(),
            version: "1.0.0".to_string(),
            functions: vec![
                FunctionSpec {
                    name: "list_invoice".to_string(),
                    is_readonly: true,
                    parameters: vec![ParameterSpec {
                        name: "id".to_string(),
                        param_type: "u64".to_string(),
                    }],
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
            types: vec![TypeSpec {
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
            }],
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
        assert!(is_readonly_fn_name("get_invoice"));
        assert!(is_readonly_fn_name("is_paused"));
        assert!(!is_readonly_fn_name("fund_invoice"));
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
        assert!(params
            .iter()
            .all(|p| !p.name.is_empty() && !p.param_type.is_empty()));
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
            functions: vec![FunctionSpec {
                name: "collect_fee".to_string(),
                is_readonly: false,
                parameters: vec![ParameterSpec {
                    name: "amount".to_string(),
                    param_type: "i128".to_string(),
                }],
                return_type: None,
            }],
            types: vec![],
        };

        let json = serde_json::to_string_pretty(&original).expect("serialization failed");
        let restored: ContractSpec = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(original, restored);
    }

    #[test]
    fn test_format_type_def_all_variants() {
        assert_eq!(format_type_def(&ScSpecTypeDef::U64), "u64");
        assert_eq!(format_type_def(&ScSpecTypeDef::I128), "i128");
        assert_eq!(format_type_def(&ScSpecTypeDef::Address), "Address");
        assert_eq!(format_type_def(&ScSpecTypeDef::Bool), "bool");
        assert_eq!(format_type_def(&ScSpecTypeDef::Symbol), "Symbol");
        assert_eq!(format_type_def(&ScSpecTypeDef::String), "String");
        assert_eq!(format_type_def(&ScSpecTypeDef::Bytes), "Bytes");
        assert_eq!(
            format_type_def(&ScSpecTypeDef::BytesN(ScSpecTypeBytesN { n: 32 })),
            "BytesN<32>"
        );

        let opt = ScSpecTypeDef::Option(Box::new(ScSpecTypeOption {
            value_type: Box::new(ScSpecTypeDef::U32),
        }));
        assert_eq!(format_type_def(&opt), "Option<u32>");

        let res = ScSpecTypeDef::Result(Box::new(ScSpecTypeResult {
            ok_type: Box::new(ScSpecTypeDef::U64),
            error_type: Box::new(ScSpecTypeDef::U32),
        }));
        assert_eq!(format_type_def(&res), "Result<u64, u32>");

        let vec_t = ScSpecTypeDef::Vec(Box::new(ScSpecTypeVec {
            element_type: Box::new(ScSpecTypeDef::String),
        }));
        assert_eq!(format_type_def(&vec_t), "Vec<String>");

        let map_t = ScSpecTypeDef::Map(Box::new(ScSpecTypeMap {
            key_type: Box::new(ScSpecTypeDef::Address),
            value_type: Box::new(ScSpecTypeDef::I128),
        }));
        assert_eq!(format_type_def(&map_t), "Map<Address, i128>");

        let tuple_t = ScSpecTypeDef::Tuple(Box::new(ScSpecTypeTuple {
            value_types: vec![ScSpecTypeDef::U64, ScSpecTypeDef::Bool]
                .try_into()
                .unwrap(),
        }));
        assert_eq!(format_type_def(&tuple_t), "(u64, bool)");

        let udt = ScSpecTypeDef::Udt(ScSpecTypeUdt {
            name: StringM::try_from("InvoiceRecord").unwrap(),
        });
        assert_eq!(format_type_def(&udt), "InvoiceRecord");
    }

    #[test]
    fn test_extract_contract_spec_from_synthetic_wasm() {
        let fn_entry = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: StringM::default(),
            name: ScSymbol(StringM::try_from("mint_invoice").unwrap()),
            inputs: VecM::try_from(vec![
                ScSpecFunctionInputV0 {
                    doc: StringM::default(),
                    name: StringM::try_from("amount").unwrap(),
                    type_: ScSpecTypeDef::I128,
                },
                ScSpecFunctionInputV0 {
                    doc: StringM::default(),
                    name: StringM::try_from("recipient").unwrap(),
                    type_: ScSpecTypeDef::Address,
                },
            ])
            .unwrap(),
            outputs: VecM::try_from(vec![ScSpecTypeDef::U64]).unwrap(),
        });

        let struct_entry = ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
            doc: StringM::default(),
            lib: StringM::default(),
            name: StringM::try_from("InvoiceMeta").unwrap(),
            fields: VecM::try_from(vec![
                ScSpecUdtStructFieldV0 {
                    doc: StringM::default(),
                    name: StringM::try_from("id").unwrap(),
                    type_: ScSpecTypeDef::U64,
                },
                ScSpecUdtStructFieldV0 {
                    doc: StringM::default(),
                    name: StringM::try_from("due_date").unwrap(),
                    type_: ScSpecTypeDef::U64,
                },
            ])
            .unwrap(),
        });

        let wasm_bytes = build_synthetic_wasm_with_spec(&[fn_entry, struct_entry]);
        assert!(wasm_bytes.starts_with(b"\0asm"));

        let spec = extract_contract_spec_from_wasm(&wasm_bytes, "invoice_nft", "1.0.0")
            .expect("extraction failed");

        assert_eq!(spec.name, "invoice_nft");
        assert_eq!(spec.version, "1.0.0");
        assert_eq!(spec.functions.len(), 1);
        assert_eq!(spec.functions[0].name, "mint_invoice");
        assert_eq!(spec.functions[0].parameters.len(), 2);
        assert_eq!(spec.functions[0].parameters[0].name, "amount");
        assert_eq!(spec.functions[0].parameters[0].param_type, "i128");
        assert_eq!(spec.functions[0].return_type, Some("u64".to_string()));
        assert!(!spec.functions[0].is_readonly);

        assert_eq!(spec.types.len(), 1);
        assert_eq!(spec.types[0].name, "InvoiceMeta");
        assert_eq!(spec.types[0].fields.len(), 2);

        let iface = spec.to_interface_txt();
        assert_eq!(
            iface,
            "fn mint_invoice(amount: i128, recipient: Address) -> u64"
        );
    }

    #[test]
    fn test_golden_spec_drift_fails_when_interface_diverges() {
        let baseline = ContractSpec {
            name: "invoice_nft".to_string(),
            version: "1.0.0".to_string(),
            functions: vec![FunctionSpec {
                name: "get_invoice".to_string(),
                is_readonly: true,
                parameters: vec![ParameterSpec {
                    name: "id".to_string(),
                    param_type: "u64".to_string(),
                }],
                return_type: Some("Invoice".to_string()),
            }],
            types: vec![],
        };

        let mut drifted = baseline.clone();
        drifted.functions[0].name = "get_invoice_v2".to_string();

        assert_ne!(baseline, drifted);
        assert_ne!(baseline.to_interface_txt(), drifted.to_interface_txt());
    }

    #[test]
    fn test_extract_from_file_path() {
        let fn_entry = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
            doc: StringM::default(),
            name: ScSymbol(StringM::try_from("get_balance").unwrap()),
            inputs: VecM::try_from(vec![]).unwrap(),
            outputs: VecM::try_from(vec![ScSpecTypeDef::I128]).unwrap(),
        });
        let wasm_bytes = build_synthetic_wasm_with_spec(&[fn_entry]);

        let tmp_dir = std::env::temp_dir();
        let wasm_path = tmp_dir.join("test_token_contract.wasm");
        fs::write(&wasm_path, wasm_bytes).expect("failed to write temp wasm");

        let spec = extract_contract_spec(&wasm_path).expect("failed to extract from path");
        assert_eq!(spec.name, "test_token_contract");
        assert_eq!(spec.functions.len(), 1);
        assert_eq!(spec.functions[0].name, "get_balance");
        assert!(spec.functions[0].is_readonly);
        assert_eq!(spec.functions[0].return_type, Some("i128".to_string()));

        let _ = fs::remove_file(wasm_path);
    }
}
