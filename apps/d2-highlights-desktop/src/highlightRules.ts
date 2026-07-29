export const HERO_KILL_RULE_ID = "hero_kill_sequence";
export const VERIFIED_TREE_CUT_RULE_ID = "verified_tree_cut";

export type HighlightRuleId =
  | typeof HERO_KILL_RULE_ID
  | typeof VERIFIED_TREE_CUT_RULE_ID;

export interface HighlightRuleDefinition {
  id: HighlightRuleId;
  label: string;
  description: string;
  aliases: readonly string[];
}

export interface HighlightRuleQueryResult {
  status: "empty" | "matched" | "unknown" | "none";
  ruleIds: HighlightRuleId[];
  unknownText: string;
}

interface CandidateLike {
  kind: string;
  interaction?: {
    pattern_id: string;
    verification?: unknown;
  };
}

interface HeroCandidateLike extends CandidateLike {
  id: string;
  primary_hero: string | null;
  start_seconds: number;
}

interface HighlightClipLike {
  candidateId: string;
  viewHero: string;
}

export interface HighlightSelectionInference {
  hero: string;
  ruleIds: HighlightRuleId[];
}

export const HIGHLIGHT_RULES: readonly HighlightRuleDefinition[] = [
  {
    id: HERO_KILL_RULE_ID,
    label: "击杀",
    description: "技能或交战起手到目标阵亡，连续击杀保持连贯",
    aliases: [
      "英雄击杀",
      "连续击杀",
      "击杀连续段",
      "击杀",
      "连杀",
      "杀人",
      "人头",
      "kills",
      "kill",
    ],
  },
  {
    id: VERIFIED_TREE_CUT_RULE_ID,
    label: "砍树",
    description: "补刀斧砍掉森海飞霞生成的临时树",
    aliases: [
      "补刀斧砍树",
      "压制之刃砍树",
      "砍掉种树",
      "砍种树",
      "砍树",
      "开树",
      "补刀斧",
      "压制之刃",
      "种树",
    ],
  },
];

export const DEFAULT_HIGHLIGHT_RULE_IDS: readonly HighlightRuleId[] =
  HIGHLIGHT_RULES.map((rule) => rule.id);

const allAliases = HIGHLIGHT_RULES.flatMap((rule) => rule.aliases).sort(
  (left, right) => right.length - left.length,
);
const exclusionPrefixes = ["不要", "不看", "排除", "去掉", "取消"];
const ignoredPhrases = [
  "我想要",
  "我想看",
  "帮我找",
  "请给我",
  "只保留",
  "只需要",
  "仅保留",
  "仅需要",
  "再加上",
  "视频内容",
  "只要",
  "只有",
  "仅看",
  "只看",
  "帮我",
  "筛选",
  "生成",
  "保留",
  "需要",
  "想要",
  "想看",
  "全部",
  "所有",
  "全选",
  "都要",
  "都看",
  "高光",
  "集锦",
  "视频",
  "片段",
  "内容",
  "一起",
  "同时",
  "不要",
  "不看",
  "排除",
  "去掉",
  "取消",
  "加上",
  "以及",
  "或者",
  "和",
  "与",
  "的",
].sort((left, right) => right.length - left.length);

export function candidateHighlightRuleId(
  candidate: CandidateLike,
): HighlightRuleId | null {
  if (candidate.kind === HERO_KILL_RULE_ID) {
    return HERO_KILL_RULE_ID;
  }
  if (
    candidate.interaction?.pattern_id ===
      "hoodwink_ground_acorn_quelling_blade" &&
    Boolean(candidate.interaction.verification)
  ) {
    return VERIFIED_TREE_CUT_RULE_ID;
  }
  return null;
}

export function normalizeHighlightRuleIds(
  ruleIds: readonly HighlightRuleId[],
): HighlightRuleId[] {
  const selected = new Set(ruleIds);
  return HIGHLIGHT_RULES.map((rule) => rule.id).filter((id) =>
    selected.has(id),
  );
}

export function filterHeroHighlightCandidates<T extends HeroCandidateLike>(
  candidates: readonly T[],
  hero: string,
  ruleIds: readonly HighlightRuleId[],
): T[] {
  const selectedRules = new Set(normalizeHighlightRuleIds(ruleIds));
  return candidates
    .filter((candidate) => {
      const ruleId = candidateHighlightRuleId(candidate);
      return (
        candidate.primary_hero === hero &&
        ruleId !== null &&
        selectedRules.has(ruleId)
      );
    })
    .sort((left, right) => left.start_seconds - right.start_seconds);
}

export function inferHighlightSelection<
  T extends HeroCandidateLike,
  C extends HighlightClipLike,
>(
  candidates: readonly T[],
  clips: readonly C[],
): HighlightSelectionInference {
  const heroes = new Set(clips.map((clip) => clip.viewHero).filter(Boolean));
  if (clips.length === 0 || heroes.size !== 1) {
    return {
      hero: "",
      ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
    };
  }

  const [hero] = heroes;
  if (!hero) {
    return {
      hero: "",
      ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
    };
  }

  const candidateById = new Map(
    candidates.map((candidate) => [candidate.id, candidate]),
  );
  const ruleIds: HighlightRuleId[] = [];
  for (const clip of clips) {
    const candidate = candidateById.get(clip.candidateId);
    const ruleId = candidate
      ? candidateHighlightRuleId(candidate)
      : null;
    if (!candidate || candidate.primary_hero !== hero || !ruleId) {
      return {
        hero: "",
        ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
      };
    }
    ruleIds.push(ruleId);
  }

  return {
    hero,
    ruleIds: normalizeHighlightRuleIds(ruleIds),
  };
}

export function formatHighlightRuleSelection(
  ruleIds: readonly HighlightRuleId[],
): string {
  const selected = new Set(ruleIds);
  return HIGHLIGHT_RULES.filter((rule) => selected.has(rule.id))
    .map((rule) => rule.label)
    .join(" + ");
}

export function parseHighlightRuleQuery(
  input: string,
): HighlightRuleQueryResult {
  const normalized = input.normalize("NFKC").toLowerCase().trim();
  if (!normalized) {
    return { status: "empty", ruleIds: [], unknownText: "" };
  }

  const compact = normalized.replace(/\s+/g, "");
  const excluded = new Set<HighlightRuleId>();
  for (const rule of HIGHLIGHT_RULES) {
    const isExcluded = rule.aliases.some((alias) =>
      exclusionPrefixes.some((prefix) =>
        compact.includes(`${prefix}${alias}`),
      ),
    );
    if (isExcluded) {
      excluded.add(rule.id);
    }
  }

  const requestsAll = ["全部", "所有", "全选", "都要", "都看"].some(
    (keyword) => compact.includes(keyword),
  );
  const matched = new Set<HighlightRuleId>(
    requestsAll
      ? DEFAULT_HIGHLIGHT_RULE_IDS
      : HIGHLIGHT_RULES.filter((rule) =>
          rule.aliases.some((alias) => compact.includes(alias)),
        ).map((rule) => rule.id),
  );
  for (const ruleId of excluded) {
    matched.delete(ruleId);
  }

  let unknownText = normalized;
  for (const alias of allAliases) {
    unknownText = unknownText.replaceAll(alias, "");
  }
  for (const phrase of ignoredPhrases) {
    unknownText = unknownText.replaceAll(phrase, "");
  }
  unknownText = unknownText
    .replace(/[\s+、，,。.!！?？/\\:：;；()（）[\]【】_-]+/g, "")
    .trim();

  if (unknownText) {
    return {
      status: "unknown",
      ruleIds: [],
      unknownText,
    };
  }

  const ruleIds = normalizeHighlightRuleIds([...matched]);
  if (ruleIds.length === 0) {
    return { status: "none", ruleIds: [], unknownText: "" };
  }
  return { status: "matched", ruleIds, unknownText: "" };
}
