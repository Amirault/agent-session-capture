//! Generator for the Warp protobuf schema lookup used by the Warp adapter.
//!
//! The Warp protos (`warpdotdev/warp-proto-apis`, AGPL-3.0) are NOT vendored in
//! this repository. This generator downloads them at the pinned revision (or
//! reads a local copy given through `PROTO_SCHEMA_DIR`), applies the same
//! preprocessing the upstream Rust build does:
//!   - `edition = "2023";` -> `syntax = "proto3";`
//!   - drop `option features.*` lines
//!   - drop the `google/protobuf/go_features.proto` import
//!   - drop `reserved` declarations
//!
//! It then parses `task.proto` and every file it imports, resolves field types the
//! way protobufjs does, and writes `generated/proto_schema.rs` (or `PROTO_OUT`):
//! a plain table `build.rs` compiles in. No protobuf dependency at runtime.
//!
//! The google well-known types come from protobufjs 8.8.0, as the original
//! TypeScript generator used them: its bundled `descriptor.proto` (downloaded at
//! that tag) and its built-in "common" definitions (reproduced below).
//!
//! Run with:
//!   cargo run --example gen-schema-lookup

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;

const SCHEMA_REV: &str = "ac1af7303d2931b0fb485be650a1fbc8b80d5667";
const UPSTREAM: &str = "https://raw.githubusercontent.com/warpdotdev/warp-proto-apis";
const UPSTREAM_DIR: &str = "apis/multi_agent/v1";
const PROTOBUFJS_DESCRIPTOR: &str =
    "https://raw.githubusercontent.com/protobufjs/protobuf.js/protobufjs-v8.8.0/google/protobuf/descriptor.proto";
const PROTO_FILES: [&str; 15] = [
    "attachment",
    "citations",
    "conversation_data",
    "document_content",
    "file_content",
    "input_context",
    "lsp",
    "options",
    "orchestration",
    "request",
    "response",
    "skill",
    "suggestions",
    "task",
    "todo",
];
const ROOT_TYPE: &str = "warp.multi_agent.v1.Task";

/// protobufjs' built-in definitions of the common well-known types
/// (`src/common.js`), as proto text. Note the camelCase `Value` fields.
fn common_definition(name: &str) -> Option<&'static str> {
    Some(match name {
        "google/protobuf/any.proto" => {
            "syntax = \"proto3\"; package google.protobuf; message Any { string type_url = 1; bytes value = 2; }"
        }
        "google/protobuf/duration.proto" => {
            "syntax = \"proto3\"; package google.protobuf; message Duration { int64 seconds = 1; int32 nanos = 2; }"
        }
        "google/protobuf/timestamp.proto" => {
            "syntax = \"proto3\"; package google.protobuf; message Timestamp { int64 seconds = 1; int32 nanos = 2; }"
        }
        "google/protobuf/empty.proto" => "syntax = \"proto3\"; package google.protobuf; message Empty { }",
        "google/protobuf/struct.proto" => {
            "syntax = \"proto3\"; package google.protobuf;
             message Struct { map<string, Value> fields = 1; }
             message Value { oneof kind { NullValue nullValue = 1; double numberValue = 2; string stringValue = 3;
               bool boolValue = 4; Struct structValue = 5; ListValue listValue = 6; } }
             enum NullValue { NULL_VALUE = 0; }
             message ListValue { repeated Value values = 1; }"
        }
        "google/protobuf/wrappers.proto" => {
            "syntax = \"proto3\"; package google.protobuf;
             message DoubleValue { double value = 1; } message FloatValue { float value = 1; }
             message Int64Value { int64 value = 1; } message UInt64Value { uint64 value = 1; }
             message Int32Value { int32 value = 1; } message UInt32Value { uint32 value = 1; }
             message BoolValue { bool value = 1; } message StringValue { string value = 1; }
             message BytesValue { bytes value = 1; }"
        }
        _ => return None,
    })
}

const SCALARS: [&str; 15] = [
    "double", "float", "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32", "fixed64", "sfixed32",
    "sfixed64", "bool", "string", "bytes",
];

fn fetch(url: &str) -> String {
    let mut response = ureq::get(url)
        .call()
        .unwrap_or_else(|e| panic!("download failed ({e}): {url}"));
    response
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_to_string()
        .expect("utf-8 proto")
}

/// Mirror the upstream build.rs preprocessing with plain string operations.
fn preprocess(content: &str) -> String {
    let kept: Vec<&str> = content
        .split('\n')
        .filter(|line| {
            let t = line.trim();
            !(t.starts_with("option features")
                || t == "import \"google/protobuf/go_features.proto\";"
                || t.starts_with("reserved ")
                || t == "reserved")
        })
        .collect();
    kept.join("\n")
        .replacen("edition = \"2023\";", "syntax = \"proto3\";", 1)
}

// ------------------------------------------------------------------ tokenizer

fn tokenize(src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            tokens.push(chars[start..i.min(chars.len())].iter().collect());
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | '-' | '+')) {
                if matches!(chars[i], '-' | '+') && i > start && !matches!(chars[i - 1], 'e' | 'E') {
                    break;
                }
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else {
            tokens.push(c.to_string());
            i += 1;
        }
    }
    tokens
}

// ------------------------------------------------------------------ parser

#[derive(Clone, Debug)]
struct FieldDef {
    name: String,
    number: u32,
    type_name: String,
    oneof: Option<String>,
}

#[derive(Default)]
struct Definitions {
    /// message full name -> (scope used for resolution, fields)
    messages: BTreeMap<String, (String, Vec<FieldDef>)>,
    enums: HashSet<String>,
    /// (extendee, scope, field)
    extensions: Vec<(String, String, FieldDef)>,
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &str {
        self.tokens.get(self.pos).map(String::as_str).unwrap_or("")
    }

    fn next(&mut self) -> String {
        let t = self.peek().to_string();
        self.pos += 1;
        t
    }

    fn expect(&mut self, wanted: &str) {
        let got = self.next();
        assert_eq!(got, wanted, "proto parse error near token {}", self.pos);
    }

    /// Skip to the end of the current statement (`;`) or balanced block.
    fn skip_statement(&mut self) {
        let mut depth = 0;
        while self.pos < self.tokens.len() {
            let t = self.next();
            match t.as_str() {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth <= 0 {
                        return;
                    }
                }
                ";" if depth == 0 => return,
                _ => {}
            }
        }
    }

    fn skip_field_options(&mut self) {
        if self.peek() == "[" {
            let mut depth = 0;
            loop {
                let t = self.next();
                if t == "[" {
                    depth += 1;
                } else if t == "]" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
        }
    }

    fn parse_file(&mut self, defs: &mut Definitions) -> Vec<String> {
        let mut package = String::new();
        let mut imports = Vec::new();
        while self.pos < self.tokens.len() {
            match self.peek() {
                "package" => {
                    self.next();
                    package = self.next();
                    self.expect(";");
                }
                "import" => {
                    self.next();
                    if matches!(self.peek(), "public" | "weak") {
                        self.next();
                    }
                    imports.push(self.next().trim_matches('"').to_string());
                    self.expect(";");
                }
                "message" => self.parse_message(&package, defs),
                "enum" => {
                    self.next();
                    let name = self.next();
                    defs.enums.insert(qualify(&package, &name));
                    self.skip_statement();
                }
                "extend" => self.parse_extend(&package, defs),
                ";" => {
                    self.next();
                }
                _ => self.skip_statement(),
            }
        }
        imports
    }

    fn parse_extend(&mut self, scope: &str, defs: &mut Definitions) {
        self.expect("extend");
        let extendee = self.next();
        self.expect("{");
        while self.peek() != "}" {
            if let Some(field) = self.parse_field(None) {
                defs.extensions.push((extendee.clone(), scope.to_string(), field));
            }
        }
        self.expect("}");
    }

    fn parse_message(&mut self, scope: &str, defs: &mut Definitions) {
        self.expect("message");
        let name = self.next();
        let full = qualify(scope, &name);
        self.expect("{");
        let mut fields = Vec::new();
        while self.peek() != "}" {
            match self.peek() {
                "message" => self.parse_message(&full, defs),
                "enum" => {
                    self.next();
                    let enum_name = self.next();
                    defs.enums.insert(qualify(&full, &enum_name));
                    self.skip_statement();
                }
                "extend" => self.parse_extend(&full, defs),
                "oneof" => {
                    self.next();
                    let oneof = self.next();
                    self.expect("{");
                    while self.peek() != "}" {
                        if self.peek() == "option" {
                            self.skip_statement();
                            continue;
                        }
                        if let Some(field) = self.parse_field(Some(oneof.clone())) {
                            fields.push(field);
                        }
                    }
                    self.expect("}");
                }
                "option" | "reserved" | "extensions" => self.skip_statement(),
                ";" => {
                    self.next();
                }
                _ => {
                    if let Some(field) = self.parse_field(None) {
                        fields.push(field);
                    }
                }
            }
        }
        self.expect("}");
        defs.messages.insert(full.clone(), (full, fields));
    }

    fn parse_field(&mut self, oneof: Option<String>) -> Option<FieldDef> {
        if self.peek() == ";" {
            self.next();
            return None;
        }
        if matches!(self.peek(), "optional" | "required" | "repeated") {
            self.next();
        }
        let type_name = if self.peek() == "map" {
            self.next();
            self.expect("<");
            let _key = self.next();
            self.expect(",");
            let value = self.next();
            self.expect(">");
            value
        } else {
            self.next()
        };
        let name = self.next();
        self.expect("=");
        let number: u32 = self.next().parse().expect("field number");
        self.skip_field_options();
        self.expect(";");
        Some(FieldDef {
            name,
            number,
            type_name,
            oneof,
        })
    }
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}.{name}")
    }
}

enum Resolved {
    Message(String),
    Leaf,
}

/// Resolve a field type from `scope` outward, as protobufjs does.
fn resolve(defs: &Definitions, type_name: &str, scope: &str) -> Resolved {
    if SCALARS.contains(&type_name) {
        return Resolved::Leaf;
    }
    let candidates: Vec<String> = if let Some(absolute) = type_name.strip_prefix('.') {
        vec![absolute.to_string()]
    } else {
        let mut out = Vec::new();
        let mut current = scope.to_string();
        loop {
            out.push(qualify(&current, type_name));
            match current.rfind('.') {
                Some(i) => current.truncate(i),
                None if !current.is_empty() => current.clear(),
                None => break,
            }
        }
        out
    };
    for candidate in candidates {
        if defs.messages.contains_key(&candidate) {
            return Resolved::Message(candidate);
        }
        if defs.enums.contains(&candidate) {
            return Resolved::Leaf;
        }
    }
    Resolved::Leaf
}

fn main() {
    let local_dir = std::env::var("PROTO_SCHEMA_DIR").ok();
    let out_path = std::env::var("PROTO_OUT").unwrap_or_else(|_| "generated/proto_schema.rs".to_string());

    let mut sources: HashMap<String, String> = HashMap::new();
    for name in PROTO_FILES {
        let text = match &local_dir {
            Some(dir) => fs::read_to_string(format!("{dir}/{name}.proto")).expect("local proto"),
            None => fetch(&format!("{UPSTREAM}/{SCHEMA_REV}/{UPSTREAM_DIR}/{name}.proto")),
        };
        sources.insert(format!("{name}.proto"), preprocess(&text));
    }
    let descriptor = match &local_dir {
        Some(dir) if fs::metadata(format!("{dir}/google/protobuf/descriptor.proto")).is_ok() => {
            fs::read_to_string(format!("{dir}/google/protobuf/descriptor.proto")).expect("descriptor")
        }
        _ => fetch(PROTOBUFJS_DESCRIPTOR),
    };
    sources.insert("google/protobuf/descriptor.proto".to_string(), descriptor);

    // Load task.proto and every file it imports (unknown imports are skipped,
    // as protobufjs skips them).
    let mut defs = Definitions::default();
    let mut queue = vec!["task.proto".to_string()];
    let mut loaded = HashSet::new();
    while let Some(file) = queue.pop() {
        if !loaded.insert(file.clone()) {
            continue;
        }
        let Some(text) = sources
            .get(&file)
            .cloned()
            .or_else(|| common_definition(&file).map(str::to_string))
        else {
            continue;
        };
        let mut parser = Parser {
            tokens: tokenize(&text),
            pos: 0,
        };
        queue.extend(parser.parse_file(&mut defs));
    }

    // Resolve every field; extensions become `.full.name` fields of their extendee.
    let mut schema: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for (full, (scope, fields)) in &defs.messages {
        let entries = schema.entry(full.clone()).or_default();
        for field in fields {
            entries.insert(field.number, field_line(&defs, field, &field.name, scope));
        }
    }
    for (extendee, scope, field) in &defs.extensions {
        let Resolved::Message(target) = resolve(&defs, extendee, scope) else {
            continue;
        };
        let name = format!(".{}", qualify(scope, &field.name));
        let line = field_line(&defs, field, &name, scope);
        schema.entry(target).or_default().insert(field.number, line);
    }
    assert!(
        schema.contains_key(ROOT_TYPE),
        "root type {ROOT_TYPE} not found in parsed schema"
    );

    let mut out = String::new();
    out.push_str("// @generated by examples/gen-schema-lookup.rs — DO NOT EDIT.\n");
    out.push_str(&format!("// Source: warpdotdev/warp-proto-apis @ {SCHEMA_REV}\n"));
    out.push_str("// Regenerate: cargo run --example gen-schema-lookup\n");
    out.push_str("// Derived from AGPL-3.0 protos: do not commit this file to a differently licensed repository.\n");
    out.push_str("// Captures message field numbers -> names (oneof variants are field-number based).\n");
    out.push_str("// Enums omitted (no varint leaves emitted by the walker).\n\n");
    out.push_str(&format!("pub const SCHEMA_REV: &str = \"{SCHEMA_REV}\";\n\n"));
    out.push_str("pub static TYPES: &[TypeEntry] = &[\n");
    for (name, fields) in &schema {
        out.push_str(&format!(
            "    TypeEntry {{\n        name: {name:?},\n        fields: &[\n"
        ));
        for line in fields.values() {
            out.push_str(&format!("            {line}\n"));
        }
        out.push_str("        ],\n    },\n");
    }
    out.push_str("];\n");
    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        fs::create_dir_all(parent).expect("output directory");
    }
    fs::write(&out_path, out).expect("write schema");
    println!("wrote {out_path} ({} types)", schema.len());
}

fn field_line(defs: &Definitions, field: &FieldDef, name: &str, scope: &str) -> String {
    let (kind, child) = match resolve(defs, &field.type_name, scope) {
        Resolved::Message(child) => ("Message", format!("Some({child:?})")),
        Resolved::Leaf => ("Leaf", "None".to_string()),
    };
    let oneof = field
        .oneof
        .as_ref()
        .map_or("None".to_string(), |o| format!("Some({o:?})"));
    format!(
        "FieldInfo {{ number: {}, name: {name:?}, kind: Kind::{kind}, child: {child}, oneof: {oneof} }},",
        field.number
    )
}
