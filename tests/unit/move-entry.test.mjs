// Run with `pnpm run test:unit` (plain Node; it strips the TypeScript types).
import { test } from "node:test";
import assert from "node:assert/strict";
import { moveEntry } from "../../src/lib/utils/profileEntries.ts";

const list = () => ["a", "b", "c", "d"];

test("move to the top: the entry goes first, the rest keep their order", () => {
    const l = list();
    assert.equal(moveEntry(l, 2, 0), true);
    assert.deepEqual(l, ["c", "a", "b", "d"]);
});

test("move to the bottom: the entry goes last, the rest keep their order", () => {
    const l = list();
    assert.equal(moveEntry(l, 1, l.length - 1), true);
    assert.deepEqual(l, ["a", "c", "d", "b"]);
});

test("an entry already at that end stays where it is", () => {
    const top = list();
    assert.equal(moveEntry(top, 0, 0), false);
    assert.deepEqual(top, list());
    const bottom = list();
    assert.equal(moveEntry(bottom, 3, 3), false);
    assert.deepEqual(bottom, list());
});

test("one step up or down, as Move Up and Move Down do", () => {
    const l = list();
    moveEntry(l, 2, 1);
    assert.deepEqual(l, ["a", "c", "b", "d"]);
    moveEntry(l, 1, 2);
    assert.deepEqual(l, list());
});

test("a drag from one place to another", () => {
    const l = list();
    moveEntry(l, 0, 2);
    assert.deepEqual(l, ["b", "c", "a", "d"]);
});

test("positions outside the list change nothing", () => {
    for (const [from, to] of [[-1, 0], [0, -1], [4, 0], [0, 4], [1.5, 0], [NaN, 0]]) {
        const l = list();
        assert.equal(moveEntry(l, from, to), false, `${from} -> ${to}`);
        assert.deepEqual(l, list());
    }
    const empty = [];
    assert.equal(moveEntry(empty, 0, 0), false);
    assert.deepEqual(empty, []);
});

test("the list is changed in place, not copied", () => {
    const l = list();
    const same = l;
    moveEntry(l, 3, 0);
    assert.equal(l, same);
    assert.deepEqual(same, ["d", "a", "b", "c"]);
});
