#!/usr/bin/env bash
# capture.sh — run agent-session-capture from any directory of your project.
#
# Usage: capture.sh --id <capture-id> --source warp|claude-code|hermes|langfuse [cli flags...]
#        capture.sh --sessions --source warp|claude-code|hermes|langfuse [cli flags...]
#
# - Runs the CLI from the caller's working directory, so relative paths resolve
#   relative to YOUR project, not to this tool.
# - Builds the release binary on first use (and again after the sources change); needs only
#   `cargo` on PATH. A binary installed with `cargo install --path .` is used when no cargo is found.
# - Defaults --out to <project>/.agent-captures, where <project> is the MAIN git checkout of
#   the caller's repository: a capture made in a disposable git worktree survives its removal
#   (the decay-safe merge reads that store). Outside git, ./.agent-captures is used.
#   --source langfuse uses its langfuse/ sub-store, so an id keeps one bundle per source
#   (and --sessions files land in langfuse/sessions/). An explicit --out wins.
# - Set AGENT_CAPTURE_CARGO_RUNNER (e.g. "mise exec rust@1.99.1 --") to run cargo through a
#   version manager, or AGENT_CAPTURE_BIN to use a prebuilt binary.
set -euo pipefail

# Resolve symlinks (bash 3.2 compatible) so `ln -s .../capture.sh ~/bin/cap` works.
SRC="${BASH_SOURCE[0]}"
while [[ -L "${SRC}" ]]; do
  link_dir="$(cd "$(dirname "${SRC}")" && pwd)"
  SRC="$(readlink "${SRC}")"
  [[ "${SRC}" != /* ]] && SRC="${link_dir}/${SRC}"
done
TOOL_DIR="$(cd "$(dirname "${SRC}")" && pwd)"

# shellcheck disable=SC2206 # intentional word splitting of the runner prefix
RUNNER=(${AGENT_CAPTURE_CARGO_RUNNER:-})

run() {
  if [[ ${#RUNNER[@]} -gt 0 ]]; then
    "${RUNNER[@]}" "$@"
  else
    "$@"
  fi
}

default_store() {
  local common_dir
  common_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [[ -n "${common_dir}" ]]; then
    # <main checkout>/.git -> <main checkout>; the common dir is the main checkout's .git.
    echo "$(dirname "${common_dir}")/.agent-captures"
  else
    echo "${PWD}/.agent-captures"
  fi
}

BIN="${AGENT_CAPTURE_BIN:-${TOOL_DIR}/target/release/agent-session-capture}"
needs_build() {
  [[ -n "${AGENT_CAPTURE_BIN:-}" ]] && return 1
  [[ ! -x "${BIN}" ]] && return 0
  # Rebuild when a source changed since the last build (e.g. after a git pull).
  [[ -n "$(find "${TOOL_DIR}/src" "${TOOL_DIR}/Cargo.toml" "${TOOL_DIR}/Cargo.lock" "${TOOL_DIR}/build.rs" \
    -newer "${BIN}" -print -quit 2>/dev/null)" ]]
}
if needs_build; then
  if [[ ${#RUNNER[@]} -eq 0 ]] && ! command -v cargo >/dev/null 2>&1; then
    if [[ ! -x "${BIN}" ]] && command -v agent-session-capture >/dev/null 2>&1; then
      BIN="$(command -v agent-session-capture)"
    elif [[ ! -x "${BIN}" ]]; then
      echo "capture.sh: cargo not found — install Rust (https://rustup.rs) or the binary (cargo install --path ${TOOL_DIR})" >&2
      exit 1
    fi
  else
    echo "capture.sh: building the release binary..." >&2
    run cargo build --release --locked --quiet --manifest-path "${TOOL_DIR}/Cargo.toml" >&2 ||
      { echo "capture.sh: cargo build failed in ${TOOL_DIR}" >&2; exit 1; }
  fi
fi

needs_default_out=1
langfuse_source=0
previous_arg=""
for arg in "$@"; do
  case "${arg}" in
    --out | --out=* | -h | --help) needs_default_out=0 ;;
    --source=langfuse) langfuse_source=1 ;;
    langfuse) if [[ "${previous_arg}" == "--source" ]]; then langfuse_source=1; fi ;;
    *) ;;
  esac
  previous_arg="${arg}"
done
if [[ "${needs_default_out}" -eq 1 ]]; then
  store="$(default_store)"
  if [[ "${langfuse_source}" -eq 1 ]]; then
    store="${store}/langfuse"
  fi
  set -- "$@" --out "${store}"
fi

exec "${BIN}" "$@"
