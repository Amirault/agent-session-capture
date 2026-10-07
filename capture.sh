#!/usr/bin/env bash
# capture.sh — run agent-session-capture from any directory of your project.
#
# Usage: capture.sh --id <capture-id> --source warp|claude-code|hermes [cli flags...]
#
# - Runs the CLI from the caller's working directory, so relative paths resolve
#   relative to YOUR project, not to this tool.
# - Installs the tool's npm dependencies on first use.
# - Defaults --out to <project>/.agent-captures, where <project> is the MAIN git checkout of
#   the caller's repository: a capture made in a disposable git worktree survives its removal
#   (the decay-safe merge reads that store). Outside git, ./.agent-captures is used.
#   An explicit --out wins.
# - Set AGENT_CAPTURE_NODE_RUNNER (e.g. "mise exec --") to run node through a version manager.
set -euo pipefail

TOOL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck disable=SC2206 # intentional word splitting of the runner prefix
RUNNER=(${AGENT_CAPTURE_NODE_RUNNER:-})

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

if [[ ! -x "${TOOL_DIR}/node_modules/.bin/tsx" ]]; then
  echo "capture.sh: installing dependencies (first run)..." >&2
  run npm ci --prefix "${TOOL_DIR}" --silent --no-audit --no-fund >&2
fi

needs_default_out=1
for arg in "$@"; do
  case "${arg}" in
    --out | --out=* | -h | --help) needs_default_out=0 ;;
    *) ;;
  esac
done
if [[ "${needs_default_out}" -eq 1 ]]; then
  set -- "$@" --out "$(default_store)"
fi

run "${TOOL_DIR}/node_modules/.bin/tsx" "${TOOL_DIR}/src/cli.ts" "$@"
