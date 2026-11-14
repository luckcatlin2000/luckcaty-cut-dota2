import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_HIGHLIGHT_RULE_IDS,
  HERO_KILL_RULE_ID,
  candidateHighlightRuleId,
  filterHeroHighlightCandidates,
  formatHighlightRuleSelection,
  inferHighlightSelection,
  normalizeHighlightRuleIds,
  parseHighlightRuleQuery,
} from "./highlightRules.ts";

test("parses supported hero-kill requests and exclusions deterministically", () => {
  assert.deepEqual(parseHighlightRuleQuery("只要击杀"), {
    status: "matched", ruleIds: [HERO_KILL_RULE_ID], unknownText: "",
  });
  assert.deepEqual(parseHighlightRuleQuery("全部高光"), {
    status: "matched", ruleIds: [HERO_KILL_RULE_ID], unknownText: "",
  });
  assert.deepEqual(parseHighlightRuleQuery("不要击杀"), {
    status: "none", ruleIds: [], unknownText: "",
  });
});

test("does not partially apply unsupported or retired requests", () => {
  for (const request of ["击杀和神箭命中", "只要砍树", "砍树和击杀", "种树"]) {
    const result = parseHighlightRuleQuery(request);
    assert.equal(result.status, "unknown");
    assert.deepEqual(result.ruleIds, []);
    assert.notEqual(result.unknownText, "");
  }
  assert.equal(parseHighlightRuleQuery("  ").status, "empty");
});

test("legacy rule IDs are discarded without silently selecting everything", () => {
  assert.deepEqual(normalizeHighlightRuleIds(["verified_tree_cut"]), []);
  assert.deepEqual(normalizeHighlightRuleIds(["verified_tree_cut", HERO_KILL_RULE_ID, HERO_KILL_RULE_ID]), [HERO_KILL_RULE_ID]);
  assert.equal(formatHighlightRuleSelection(DEFAULT_HIGHLIGHT_RULE_IDS), "击杀");
  assert.equal(candidateHighlightRuleId({ kind: HERO_KILL_RULE_ID }), HERO_KILL_RULE_ID);
  assert.equal(candidateHighlightRuleId({ kind: "mechanical_counterplay" }), null);
  assert.equal(candidateHighlightRuleId({ kind: "team_fight" }), null);
});

test("filters only the selected hero's kills in timeline order", () => {
  const candidates = [
    { id: "late", kind: HERO_KILL_RULE_ID, primary_hero: "mirana", start_seconds: 40 },
    { id: "generic", kind: "team_fight", primary_hero: "mirana", start_seconds: 15 },
    { id: "early", kind: HERO_KILL_RULE_ID, primary_hero: "mirana", start_seconds: 10 },
    { id: "other", kind: HERO_KILL_RULE_ID, primary_hero: "axe", start_seconds: 5 },
  ];
  assert.deepEqual(filterHeroHighlightCandidates(candidates, "mirana", DEFAULT_HIGHLIGHT_RULE_IDS).map(candidate => candidate.id), ["early", "late"]);
  assert.deepEqual(filterHeroHighlightCandidates(candidates, "mirana", []).map(candidate => candidate.id), []);
});

test("restores kill selections but never converts retired clips into kill selections", () => {
  const candidates = [
    { id: "kill", kind: HERO_KILL_RULE_ID, primary_hero: "mirana", start_seconds: 10 },
    { id: "retired", kind: "mechanical_counterplay", primary_hero: "mirana", start_seconds: 20 },
  ];
  assert.deepEqual(inferHighlightSelection(candidates, [{ candidateId: "kill", viewHero: "mirana" }]), {
    hero: "mirana", ruleIds: [HERO_KILL_RULE_ID],
  });
  for (const clips of [
    [{ candidateId: "retired", viewHero: "mirana" }],
    [{ candidateId: "kill", viewHero: "mirana" }, { candidateId: "retired", viewHero: "mirana" }],
    [{ candidateId: "missing", viewHero: "mirana" }],
    [{ candidateId: "kill", viewHero: "mirana" }, { candidateId: "retired", viewHero: "axe" }],
  ]) {
    assert.deepEqual(inferHighlightSelection(candidates, clips), { hero: "", ruleIds: [] });
  }
});
