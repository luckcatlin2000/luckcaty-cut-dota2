import assert from "node:assert/strict";
import test from "node:test";

import { preferredAnalysisExportDirectory } from "./analysisExportDirectory.ts";

test("uses the video project or highlight root instead of the replay directory", () => {
  assert.equal(
    preferredAnalysisExportDirectory("", "D:\\VideoProject\\assets\\incoming"),
    "D:\\VideoProject\\assets\\incoming",
  );
});

test("keeps the directory explicitly chosen by the user", () => {
  assert.equal(
    preferredAnalysisExportDirectory(
      "D:\\我的分析包",
      "D:\\HighlightApp",
    ),
    "D:\\我的分析包",
  );
});
