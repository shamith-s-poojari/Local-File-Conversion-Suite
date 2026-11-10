// Same cases as workers/sdk/tests/test_pdf.py, so both parsers agree.
import { expect, test } from "vitest";
import { parsePageRanges } from "./pageRanges";

test.each([
  ["1-3, 5, 8-", [[1, 3], [5, 5], [8, 10]]],
  ["-2", [[1, 2]]],
  [" 4 ", [[4, 4]]],
  ["10-10,1", [[10, 10], [1, 1]]],
])("%s parses", (spec, ranges) => {
  expect(parsePageRanges(spec, 10)).toEqual({ ok: true, ranges });
});

test.each([
  ["", "empty"],
  ["0", "outOfBounds"],
  ["11", "outOfBounds"],
  ["3-2", "reversed"],
  ["1-2-3", "syntax"],
  ["x", "syntax"],
  ["-", "syntax"],
  ["1".repeat(10_001), "tooLong"],
])("%s is rejected as %s", (spec, reason) => {
  expect(parsePageRanges(spec, 10)).toEqual({ ok: false, reason });
});
