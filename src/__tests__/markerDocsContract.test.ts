import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseMarker } from "../adapters/readers/markerReader.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const MARKER_LINE = /^: CAPTURE_MARKER v=1 id=\S+( label=\S+)?$/gm;

describe("documented markers stay parseable", () => {
  it.each(["README.md", "docs/MARKER.md", "skills/capture-marker/SKILL.md"])(
    "Given %s, When its concrete marker examples are parsed, Then the tool binds every one",
    (file) => {
      const content = fs.readFileSync(path.join(ROOT, file), "utf8");
      const concrete = [...content.matchAll(MARKER_LINE)]
        .map((m) => m[0])
        .filter((line) => !line.includes("<"));

      expect(concrete.length, `${file} must show a concrete marker`).toBeGreaterThan(0);
      for (const line of concrete) {
        expect(parseMarker(line), line).not.toBeNull();
      }
    }
  );

  it("Given the marker skill, When its capture commands are inspected, Then each selects the runtime source explicitly", () => {
    const content = fs
      .readFileSync(path.join(ROOT, "skills/capture-marker/SKILL.md"), "utf8")
      .replace(/\s+/g, " ");
    const commands = content.split("capture.sh --id").length - 1;

    expect(commands).toBeGreaterThan(0);
    expect(content.split("--source <session_source>").length - 1).toBe(commands);
    expect(content).toContain("Warp → `warp`, Claude Code → `claude-code`, Hermes → `hermes`");
  });
});
