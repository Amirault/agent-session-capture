//! Compiles the locally generated Warp schema lookup in when present.
//!
//! `cargo run --example gen-schema-lookup` writes `generated/proto_schema.rs`
//! (AGPL-derived, never committed). Without it an empty table is used.

use std::{env, fs, path::Path};

const EMPTY: &str = "pub const SCHEMA_REV: &str = \"ac1af7303d2931b0fb485be650a1fbc8b80d5667\";\n\
#[allow(dead_code)]\nconst _UNUSED: Option<(FieldInfo, Kind)> = None;\n\
pub static TYPES: &[TypeEntry] = &[];\n";

fn main() {
    let generated = env::var("PROTO_SCHEMA_RS").unwrap_or_else(|_| "generated/proto_schema.rs".to_string());
    println!("cargo:rerun-if-changed={generated}");
    println!("cargo:rerun-if-env-changed=PROTO_SCHEMA_RS");
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("proto_schema.rs");
    let content = fs::read_to_string(&generated).unwrap_or_else(|_| EMPTY.to_string());
    fs::write(out, content).expect("write proto_schema.rs");
}
