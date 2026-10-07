// Placeholder. The real lookup is generated from the Warp protos (AGPL-3.0, not vendored here):
//   npm run gen:schema
// With this empty schema every Warp protobuf path keeps its numbered form and carries
// `meta.confidence: "schema-mismatch"` — correct, just less readable. Claude Code and Hermes
// sources never use it.
export const SCHEMA_REV = "ac1af7303d2931b0fb485be650a1fbc8b80d5667";

export interface FieldInfo {
  name: string;
  kind: "message" | "leaf";
  child?: string;
  oneof?: string;
}

export type Schema = Record<string, Record<number, FieldInfo>>;

export const ROOT_TYPE = "warp.multi_agent.v1.Task";

export const PROTO_SCHEMA: Schema = {};
