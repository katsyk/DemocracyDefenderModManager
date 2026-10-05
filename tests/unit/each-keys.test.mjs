// Run with `pnpm run test:unit` (plain Node; it strips the TypeScript types).
import { test } from "node:test";
import assert from "node:assert/strict";
import { updateEntryKey, withUniqueKeys } from "../../src/lib/utils/eachKeys.ts";

test("repeated keys get a suffix, so a keyed {#each} never sees a duplicate", () => {
    const items = ["a", "b", "a", "a", "c", "b"];
    const keyed = withUniqueKeys(items, x => x);
    assert.deepEqual(keyed.map(([, k]) => k), ["a", "b", "a#1", "a#2", "c", "b#1"]);
    assert.deepEqual(keyed.map(([x]) => x), items);
    assert.equal(new Set(keyed.map(([, k]) => k)).size, items.length);
    assert.deepEqual(withUniqueKeys([], x => x), []);
});

test("update results of one mod on one site with two IDs have different keys", () => {
    const e = (SourceId) => ({ Guid: "g1", Provider: "nexus", SourceId });
    assert.notEqual(updateEntryKey(e("100")), updateEntryKey(e("200")));
    assert.equal(updateEntryKey(e("100")), updateEntryKey(e("100")));
    assert.notEqual(updateEntryKey({ Guid: "g1", Provider: "nexus" }), updateEntryKey(e("1")));
    // Even identical results (no ID sent) can be listed.
    const keys = withUniqueKeys([e(undefined), e(undefined)], updateEntryKey).map(([, k]) => k);
    assert.equal(new Set(keys).size, 2);
});
