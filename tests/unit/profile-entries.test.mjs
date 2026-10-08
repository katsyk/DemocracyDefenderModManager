// Run with `pnpm run test:unit` (plain Node; it strips the TypeScript types).
import { test } from "node:test";
import assert from "node:assert/strict";
import { configFits, deployableEntries, fitConfig, defaultConfigFor, removeDuplicateEntries, removeEntriesOf, renameEntries } from "../../src/lib/utils/profileEntries.ts";

const v1 = (options) => ({ Version: 1, Guid: "g1", Name: "M", Description: "", Options: options });
const opt = (subs = 0) => ({ Name: "o", Description: "", SubOptions: subs ? Array.from({ length: subs }, () => ({ Name: "s", Description: "", Include: [] })) : undefined });

test("a profile whose mods are all missing has nothing to deploy", () => {
    const configs = [{ For: "V1", Guid: "a", Enabled: true, Toggled: [], Selected: [] }, { For: "Legacy", Guid: "b", Enabled: true, Selected: 0 }];
    assert.deepEqual(deployableEntries(configs, []), { loaded: [], missing: 2 });
    const some = deployableEntries(configs, ["b"]);
    assert.equal(some.loaded.length, 1);
    assert.equal(some.missing, 1);
});

test("stale option choices are reset to the defaults, keeping on/off", () => {
    const manifest = v1([opt(2), opt()]);
    const good = { For: "V1", Guid: "g1", Enabled: false, Toggled: [true, false], Selected: [1, 0] };
    assert.ok(configFits(good, manifest));
    assert.deepEqual(fitConfig(good, manifest), { config: good, reset: false });

    // The mod came back with one option fewer.
    const fewer = v1([opt(2)]);
    const fitted = fitConfig(good, fewer);
    assert.equal(fitted.reset, true);
    assert.deepEqual(fitted.config, { ...defaultConfigFor(fewer), Enabled: false });

    // A sub-option index that no longer exists.
    assert.ok(!configFits({ ...good, Selected: [2, 0] }, manifest));
    // Another manifest version entirely.
    assert.ok(!configFits(good, { ...manifest, Version: 2 }));
    assert.ok(!configFits({ For: "Legacy", Guid: "g1", Enabled: true, Selected: 0 }, manifest));
    // Legacy: the selected option must exist (or be 0 with none).
    const legacy = { Guid: "g1", Name: "L", Description: "", Options: ["a", "b"] };
    assert.ok(configFits({ For: "Legacy", Guid: "g1", Enabled: true, Selected: 1 }, legacy));
    assert.ok(!configFits({ For: "Legacy", Guid: "g1", Enabled: true, Selected: 2 }, legacy));
    assert.ok(configFits({ For: "Legacy", Guid: "g1", Enabled: true, Selected: 0 }, { ...legacy, Options: undefined }));
});

test("a deleted mod leaves every profile, all of its entries, in place", () => {
    const entry = (guid) => ({ For: "V1", Guid: guid, Enabled: true, Toggled: [], Selected: [] });
    const configs = [entry("gone"), entry("keep"), entry("gone")];
    const shown = configs;
    assert.equal(removeEntriesOf(configs, "gone"), 2);
    assert.deepEqual(configs.map(c => c.Guid), ["keep"]);
    // The same array, so a list shown from it updates too.
    assert.equal(shown, configs);
    assert.equal(removeEntriesOf(configs, "gone"), 0);
    assert.equal(removeEntriesOf([], "gone"), 0);
});

test("repeated entries of a mod are removed, keeping the first (issue #71)", () => {
    const e = (guid, enabled = true) => ({ For: "V1", Guid: guid, Enabled: enabled, Toggled: [], Selected: [] });
    const configs = [e("a"), e("b"), e("a", false), e("c"), e("B"), e("a")];
    const first = configs[0];
    assert.deepEqual(removeDuplicateEntries(configs), ["a", "B"]);
    assert.deepEqual(configs.map(c => c.Guid), ["a", "b", "c"]);
    // In place, and the kept entry is the first one (its options and on/off).
    assert.equal(configs[0], first);
    assert.deepEqual(removeDuplicateEntries(configs), []);
    assert.deepEqual(removeDuplicateEntries([]), []);
});

test("a mod's entries follow it to a new ID, keeping on/off and position", () => {
    const configs = [
        { For: "V1", Guid: "a", Enabled: true, Toggled: [], Selected: [] },
        { For: "Legacy", Guid: "LOCAL-old", Enabled: false, Selected: 1 },
        { For: "V1", Guid: "b", Enabled: true, Toggled: [], Selected: [] },
    ];
    assert.equal(renameEntries(configs, "local-OLD", "g1"), 1);
    assert.deepEqual(configs.map(c => c.Guid), ["a", "g1", "b"]);
    assert.equal(configs[1].Enabled, false);
    const fitted = fitConfig(configs[1], v1([opt(2), opt()]));
    assert.ok(fitted.reset);
    assert.deepEqual(fitted.config, { For: "V1", Guid: "g1", Enabled: false, Toggled: [true, true], Selected: [0, 0] });
    assert.equal(renameEntries(configs, "LOCAL-old", "g1"), 0, "nothing left to move");
});

test("a list that already has the new ID keeps the first entry", () => {
    const configs = [
        { For: "V1", Guid: "g1", Enabled: true, Toggled: [], Selected: [] },
        { For: "Legacy", Guid: "old", Enabled: false, Selected: 0 },
    ];
    assert.equal(renameEntries(configs, "old", "g1"), 1);
    assert.deepEqual(configs.map(c => [c.Guid, c.Enabled]), [["g1", true]]);
});
