/**
 * Generator for the protobuf schema lookup used by the Warp adapter.
 *
 * The Warp protos (`warpdotdev/warp-proto-apis`, AGPL-3.0) are NOT vendored in this
 * repository. This script downloads them at the pinned revision (or reads a local copy
 * given through PROTO_SCHEMA_DIR), applies the SAME preprocessing the upstream Rust build does (build.rs):
 *   - `edition = "2023";` -> `syntax = "proto3";`
 *   - drop `option features.*` lines
 *   - drop the `google/protobuf/go_features.proto` import
 *   - drop `reserved` declarations (proto3 wants bare names quoted; reserved
 *     fields carry no signal for the overlay, so strip rather than quote)
 * then reflects the parsed schema with protobufjs and emits
 * `src/adapters/protoSchema.ts` — a plain TS constant the runtime overlay
 * reads. No protobuf dependency at runtime.
 *
 * Run with:
 *   npm run gen:schema
 */
import protobuf from "protobufjs";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
/** Path to protobufjs's bundled google well-known protos. */
const BUNDLED_GOOGLE_DIR = path.join(
  path.dirname(require.resolve("protobufjs/package.json")),
  "google",
  "protobuf"
);

const LOCAL_SCHEMA_DIR = process.env.PROTO_SCHEMA_DIR;
const OUT_PATH = process.env.PROTO_OUT
  ?? path.resolve(import.meta.dirname, "..", "src", "adapters", "protoSchema.ts");
const SCHEMA_REV = "ac1af7303d2931b0fb485be650a1fbc8b80d5667";
const UPSTREAM_REPO = "warpdotdev/warp-proto-apis";
const UPSTREAM_DIR = "apis/multi_agent/v1";
const PROTO_FILES = [
  "attachment", "citations", "conversation_data", "document_content", "file_content",
  "input_context", "lsp", "options", "orchestration", "request", "response", "skill",
  "suggestions", "task", "todo",
];

/** Download the pinned protos into a fresh temp directory. */
async function downloadProtos(): Promise<string> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "warp-protos-"));
  for (const name of PROTO_FILES) {
    const url = `https://raw.githubusercontent.com/${UPSTREAM_REPO}/${SCHEMA_REV}/${UPSTREAM_DIR}/${name}.proto`;
    const res = await fetch(url);
    if (!res.ok) throw new Error(`download failed (${res.status}): ${url}`);
    fs.writeFileSync(path.join(dir, `${name}.proto`), await res.text());
  }
  return dir;
}

type FieldInfo = {
  name: string;
  kind: "message" | "leaf";
  child?: string;
  oneof?: string;
};
type Schema = Record<string, Record<number, FieldInfo>>;

/** Mirror build.rs preprocessing with plain string ops (no regex). */
function preprocess(content: string): string {
  const keepLines = content.split("\n").filter((line) => {
    const trimmed = line.trim();
    if (trimmed.startsWith("option features")) return false;
    if (trimmed === 'import "google/protobuf/go_features.proto";') return false;
    if (trimmed.startsWith("reserved ") || trimmed === "reserved") return false;
    return true;
  });
  return keepLines.join("\n").replace('edition = "2023";', 'syntax = "proto3";');
}

function collectTypes(namespace: protobuf.NamespaceBase, schema: Schema): void {
  for (const nested of namespace.nestedArray) {
    if (nested instanceof protobuf.Type) {
      const fields: Record<number, FieldInfo> = {};
      for (const field of nested.fieldsArray) {
        const isMessage = field.resolvedType instanceof protobuf.Type;
        fields[field.id] = {
          name: field.name,
          kind: isMessage ? "message" : "leaf",
          child: isMessage
            ? (field.resolvedType as protobuf.Type).fullName.replace(/^\./, "")
            : undefined,
          oneof: field.partOf ? field.partOf.name : undefined,
        };
      }
      schema[nested.fullName.replace(/^\./, "")] = fields;
      collectTypes(nested, schema);
    } else if (nested instanceof protobuf.Namespace) {
      collectTypes(nested, schema);
    }
    // Enums intentionally not captured: oneof variants are field-number based
    // and the walker emits no varint leaves, so enum values carry no signal.
  }
}

async function main(): Promise<void> {
  const downloaded = LOCAL_SCHEMA_DIR === undefined ? await downloadProtos() : undefined;
  const SCHEMA_DIR = LOCAL_SCHEMA_DIR ?? downloaded!;
  if (!fs.existsSync(SCHEMA_DIR)) {
    throw new Error(`schema dir not found: ${SCHEMA_DIR}`);
  }

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "proto-schema-"));
  try {
    for (const entry of fs.readdirSync(SCHEMA_DIR)) {
      if (!entry.endsWith(".proto")) continue;
      const src = fs.readFileSync(path.join(SCHEMA_DIR, entry), "utf8");
      fs.writeFileSync(path.join(tmp, entry), preprocess(src));
    }

    // Copy protobufjs's bundled google well-known protos (descriptor, timestamp,
    // duration, struct, empty, ...) so cross-file imports resolve without protoc.
    const tmpGoogle = path.join(tmp, "google", "protobuf");
    fs.mkdirSync(tmpGoogle, { recursive: true });
    for (const entry of fs.readdirSync(BUNDLED_GOOGLE_DIR)) {
      if (!entry.endsWith(".proto")) continue;
      fs.copyFileSync(path.join(BUNDLED_GOOGLE_DIR, entry), path.join(tmpGoogle, entry));
    }

    const root = new protobuf.Root();
    root.loadSync(path.join(tmp, "task.proto"), { keepCase: true });
    root.resolveAll();

    const schema: Schema = {};
    collectTypes(root, schema);

    const taskKey = "warp.multi_agent.v1.Task";
    if (!schema[taskKey]) {
      throw new Error(`root type ${taskKey} not found in parsed schema`);
    }

    const sorted: Schema = {};
    for (const key of Object.keys(schema).sort()) {
      const fields = schema[key];
      const sortedFields: Record<number, FieldInfo> = {};
      for (const num of Object.keys(fields).map(Number).sort((a, b) => a - b)) {
        sortedFields[num] = fields[num];
      }
      sorted[key] = sortedFields;
    }

    const header = [
      "// @generated by scripts/gen-schema-lookup.ts — DO NOT EDIT.",
      `// Source: warpdotdev/warp-proto-apis @ ${SCHEMA_REV}`,
      "// Regenerate: npm run gen:schema",
      "// Derived from AGPL-3.0 protos: do not commit this file to a differently licensed repository.",
      "// Captures message field numbers -> names (oneof variants are field-number based).",
      "// Enums omitted (no varint leaves emitted by the walker).",
      "",
    ].join("\n");

    const body = [
      `export const SCHEMA_REV = ${JSON.stringify(SCHEMA_REV)};`,
      "",
      "export interface FieldInfo {",
      "  name: string;",
      '  kind: "message" | "leaf";',
      "  child?: string;",
      "  oneof?: string;",
      "}",
      "",
      "export type Schema = Record<string, Record<number, FieldInfo>>;",
      "",
      `export const ROOT_TYPE = ${JSON.stringify(taskKey)};`,
      "",
      "export const PROTO_SCHEMA: Schema = " + JSON.stringify(sorted, null, 2) + ";",
      "",
    ].join("\n");

    fs.mkdirSync(path.dirname(OUT_PATH), { recursive: true });
    fs.writeFileSync(OUT_PATH, header + body);
    console.log(`wrote ${OUT_PATH} (${Object.keys(sorted).length} types)`);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
    if (downloaded !== undefined) fs.rmSync(downloaded, { recursive: true, force: true });
  }
}

main().catch((err: unknown) => {
  console.error(err);
  process.exit(1);
});
