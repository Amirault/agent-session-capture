//! Field-number → name lookup for Warp's protobuf `Task` schema.
//!
//! The Warp protos (`warpdotdev/warp-proto-apis`) are AGPL-3.0, so neither the
//! `.proto` files nor the generated table are vendored: the table is generated
//! locally (`cargo run --example gen-schema-lookup`) into
//! `generated/proto_schema.rs`, which `build.rs` compiles in when present.
//! Without it the table is empty and every Warp protobuf path keeps its
//! numbered form with `meta.confidence: "schema-mismatch"` — correct, just
//! less readable. Claude Code, Hermes and Langfuse never use it.

/// Whether a field holds a nested message or a leaf (scalar, string, enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Message,
    Leaf,
}

/// One field of a message type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub number: u32,
    pub name: &'static str,
    pub kind: Kind,
    /// Full name of the nested message type (message fields only).
    pub child: Option<&'static str>,
    /// Name of the oneof the field belongs to, if any.
    pub oneof: Option<&'static str>,
}

/// One message type and its fields (sorted by number).
#[derive(Clone, Copy, Debug)]
pub struct TypeEntry {
    pub name: &'static str,
    pub fields: &'static [FieldInfo],
}

/// A schema: message types sorted by full name.
#[derive(Clone, Copy, Debug)]
pub struct Schema {
    pub types: &'static [TypeEntry],
}

impl Schema {
    pub fn field(&self, type_name: &str, number: u32) -> Option<&'static FieldInfo> {
        let i = self.types.binary_search_by(|t| t.name.cmp(type_name)).ok()?;
        let fields = self.types[i].fields;
        fields
            .binary_search_by(|f| f.number.cmp(&number))
            .ok()
            .map(|j| &fields[j])
    }
}

pub const ROOT_TYPE: &str = "warp.multi_agent.v1.Task";

mod generated {
    use super::{FieldInfo, Kind, TypeEntry};
    include!(concat!(env!("OUT_DIR"), "/proto_schema.rs"));
}

/// The pinned upstream revision of the Warp protos.
pub use generated::SCHEMA_REV;

/// The schema compiled into this binary (empty unless generated locally).
pub static PROTO_SCHEMA: Schema = Schema {
    types: generated::TYPES,
};

/// The schema the overlay uses: the generated one, or under `cargo test` the
/// small hand-trimmed test schema (as the original vitest alias did).
pub fn active_schema() -> &'static Schema {
    #[cfg(test)]
    {
        &crate::tests::fixtures::test_proto_schema::TEST_SCHEMA
    }
    #[cfg(not(test))]
    {
        &PROTO_SCHEMA
    }
}
