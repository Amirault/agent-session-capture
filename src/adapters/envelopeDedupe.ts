import {
  summarizeCollapsedNode,
  type CollapsedNode,
  type SkillSummary,
} from "./collapseDeltas.js";

const PROJECT_RULES_PREFIX = "context.project_rules.";
const SIGNAL_MESSAGE_KINDS: ReadonlySet<string> = new Set([
  "agent_reasoning",
  "messages_received_from_agents",
  "update_todos",
  "user_query",
]);

export interface EnvelopeDedupeState {
  seenProjectRules: Set<string>;
  seenSkillSets: Set<string>;
}

export function createEnvelopeDedupeState(): EnvelopeDedupeState {
  return {
    seenProjectRules: new Set<string>(),
    seenSkillSets: new Set<string>(),
  };
}

/**
 * Keep one copy of static project rules and skill context across task rows.
 * Signal fields on a mixed tool event remain untouched; only the repeated
 * envelope payload is removed.
 */
export function dedupeEnvelopes(
  nodes: readonly CollapsedNode[],
  state: EnvelopeDedupeState
): CollapsedNode[] {
  return nodes.flatMap((node) => dedupeEnvelope(node, state));
}

function dedupeEnvelope(
  node: CollapsedNode,
  state: EnvelopeDedupeState
): CollapsedNode[] {
  if (
    node.message_kind !== undefined &&
    SIGNAL_MESSAGE_KINDS.has(node.message_kind)
  ) {
    return [{ ...node }];
  }

  const fields = { ...(node.fields ?? {}) };
  const projectRuleEntries = Object.entries(fields)
    .filter(([field]) => field.startsWith(PROJECT_RULES_PREFIX))
    .sort(([left], [right]) => left.localeCompare(right));
  let projectRulesRemoved = false;
  if (projectRuleEntries.length > 0) {
    const key = JSON.stringify(projectRuleEntries);
    if (state.seenProjectRules.has(key)) {
      for (const [field] of projectRuleEntries) delete fields[field];
      projectRulesRemoved = true;
    } else {
      state.seenProjectRules.add(key);
    }
  }

  let skills = node.skills;
  let skillsRemoved = false;
  if (skills !== undefined && skills.length > 0) {
    const key = skillSetKey(skills);
    if (state.seenSkillSets.has(key)) {
      skills = undefined;
      skillsRemoved = true;
    } else {
      state.seenSkillSets.add(key);
    }
  }

  if (!projectRulesRemoved && !skillsRemoved) return [{ ...node }];

  const remainingSkills = skills ?? [];
  if (Object.keys(fields).length === 0 && remainingSkills.length === 0) return [];

  const { fields: _fields, skills: _skills, ...rest } = node;
  return [
    {
      ...rest,
      ...(Object.keys(fields).length > 0 ? { fields } : {}),
      ...(remainingSkills.length > 0 ? { skills: remainingSkills } : {}),
      value: summarizeCollapsedNode(fields, remainingSkills),
    },
  ];
}

function skillSetKey(skills: readonly SkillSummary[]): string {
  const identities = skills
    .map((skill) => [skill.path ?? null, skill.name ?? null])
    .sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
  return JSON.stringify(identities);
}
