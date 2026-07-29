import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_HIGHLIGHT_RULE_IDS,
  HERO_KILL_RULE_ID,
  VERIFIED_TREE_CUT_RULE_ID,
  candidateHighlightRuleId,
  filterHeroHighlightCandidates,
  formatHighlightRuleSelection,
  inferHighlightSelection,
  normalizeHighlightRuleIds,
  parseHighlightRuleQuery,
} from "./highlightRules.ts";

test("parses common Chinese highlight requests deterministically", () => {
  assert.deepEqual(parseHighlightRuleQuery("只要击杀"), {
    status: "matched",
    ruleIds: [HERO_KILL_RULE_ID],
    unknownText: "",
  });
  assert.deepEqual(parseHighlightRuleQuery("只要砍树"), {
    status: "matched",
    ruleIds: [VERIFIED_TREE_CUT_RULE_ID],
    unknownText: "",
  });
  assert.deepEqual(parseHighlightRuleQuery("砍树和击杀"), {
    status: "matched",
    ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
    unknownText: "",
  });
  assert.deepEqual(parseHighlightRuleQuery("不要砍树，只看击杀"), {
    status: "matched",
    ruleIds: [HERO_KILL_RULE_ID],
    unknownText: "",
  });
});

test("does not partially apply an unknown request", () => {
  assert.deepEqual(parseHighlightRuleQuery("击杀和神箭命中"), {
    status: "unknown",
    ruleIds: [],
    unknownText: "神箭命中",
  });
  assert.equal(parseHighlightRuleQuery("  ").status, "empty");
  assert.equal(
    parseHighlightRuleQuery("不要击杀，不要砍树").status,
    "none",
  );
});

test("classifies only registered and verified candidate types", () => {
  assert.equal(
    candidateHighlightRuleId({ kind: HERO_KILL_RULE_ID }),
    HERO_KILL_RULE_ID,
  );
  assert.equal(
    candidateHighlightRuleId({
      kind: "interaction",
      interaction: {
        pattern_id: "hoodwink_ground_acorn_quelling_blade",
        verification: { method: "entity_lifecycle" },
      },
    }),
    VERIFIED_TREE_CUT_RULE_ID,
  );
  assert.equal(
    candidateHighlightRuleId({
      kind: "interaction",
      interaction: {
        pattern_id: "hoodwink_ground_acorn_quelling_blade",
      },
    }),
    null,
  );
  assert.equal(candidateHighlightRuleId({ kind: "team_fight" }), null);
});

test("normalizes rule order and provides a compact label", () => {
  const normalized = normalizeHighlightRuleIds([
    VERIFIED_TREE_CUT_RULE_ID,
    HERO_KILL_RULE_ID,
    VERIFIED_TREE_CUT_RULE_ID,
  ]);
  assert.deepEqual(normalized, [...DEFAULT_HIGHLIGHT_RULE_IDS]);
  assert.equal(formatHighlightRuleSelection(normalized), "击杀 + 砍树");
});

test("filters one hero by selected rules and keeps timeline order", () => {
  const candidates = [
    {
      id: "tree",
      kind: "interaction",
      primary_hero: "windrunner",
      start_seconds: 20,
      interaction: {
        pattern_id: "hoodwink_ground_acorn_quelling_blade",
        verification: { method: "entity_lifecycle" },
      },
    },
    {
      id: "kill",
      kind: HERO_KILL_RULE_ID,
      primary_hero: "windrunner",
      start_seconds: 10,
    },
    {
      id: "other-hero",
      kind: HERO_KILL_RULE_ID,
      primary_hero: "mirana",
      start_seconds: 5,
    },
  ];

  assert.deepEqual(
    filterHeroHighlightCandidates(candidates, "windrunner", [
      HERO_KILL_RULE_ID,
    ]).map((candidate) => candidate.id),
    ["kill"],
  );
  assert.deepEqual(
    filterHeroHighlightCandidates(candidates, "windrunner", [
      VERIFIED_TREE_CUT_RULE_ID,
    ]).map((candidate) => candidate.id),
    ["tree"],
  );
  assert.deepEqual(
    filterHeroHighlightCandidates(
      candidates,
      "windrunner",
      DEFAULT_HIGHLIGHT_RULE_IDS,
    ).map((candidate) => candidate.id),
    ["kill", "tree"],
  );
});

test("infers hero and exact rules from restored clips", () => {
  const candidates = [
    {
      id: "tree",
      kind: "mechanical_counterplay",
      primary_hero: "windrunner",
      start_seconds: 20,
      interaction: {
        pattern_id: "hoodwink_ground_acorn_quelling_blade",
        verification: { method: "entity_lifecycle" },
      },
    },
    {
      id: "kill",
      kind: HERO_KILL_RULE_ID,
      primary_hero: "windrunner",
      start_seconds: 10,
    },
    {
      id: "generic",
      kind: "team_fight",
      primary_hero: "windrunner",
      start_seconds: 30,
    },
  ];

  assert.deepEqual(
    inferHighlightSelection(candidates, [
      { candidateId: "tree", viewHero: "windrunner" },
    ]),
    {
      hero: "windrunner",
      ruleIds: [VERIFIED_TREE_CUT_RULE_ID],
    },
  );
  assert.deepEqual(
    inferHighlightSelection(candidates, [
      { candidateId: "kill", viewHero: "windrunner" },
      { candidateId: "tree", viewHero: "windrunner" },
    ]),
    {
      hero: "windrunner",
      ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
    },
  );
  assert.deepEqual(
    inferHighlightSelection(candidates, [
      { candidateId: "generic", viewHero: "windrunner" },
    ]),
    {
      hero: "",
      ruleIds: [...DEFAULT_HIGHLIGHT_RULE_IDS],
    },
  );
});
