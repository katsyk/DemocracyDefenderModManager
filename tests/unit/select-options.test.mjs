// Run with `pnpm run test:unit` (plain Node; it strips the TypeScript types).
//
// Regression tests for #55: a mod's sub-option dropdowns were empty in
// webviews without customizable-select support, because Select.svelte
// wrapped its <option>s in a <div>. Those webviews only list an option that
// is a child of the <select> (or of an <optgroup>).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parse } from "svelte/compiler";
import { hasSubOptionChoice } from "../../src/lib/utils/profileEntries.ts";

const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");

/** Every element in a Svelte AST fragment, with the elements above it. */
function* elements(fragment, ancestors = []) {
    for (const node of fragment?.nodes ?? []) {
        const isElement = node.type === "RegularElement";
        if (isElement) yield { node, ancestors };
        const next = isElement ? [...ancestors, node] : ancestors;
        for (const key of ["fragment", "body", "consequent", "alternate", "pending", "then", "catch"]) {
            if (node[key]?.nodes) yield* elements(node[key], next);
        }
    }
}

test("Select.svelte's <option>s are direct children of its <select>", () => {
    const ast = parse(read("src/lib/components/Select.svelte"), { modern: true });
    const all = [...elements(ast.fragment)];
    const options = all.filter(({ node }) => node.name === "option");
    assert.ok(options.length > 0, "Select.svelte renders <option>s");
    for (const { ancestors } of options) {
        const parent = ancestors.at(-1);
        assert.ok(
            parent && (parent.name === "select" || parent.name === "optgroup"),
            `an <option> sits inside <${parent?.name}>, which legacy webviews don't list`,
        );
    }
});

test("Select.svelte gives options a plain-text label for legacy webviews", () => {
    const source = read("src/lib/components/Select.svelte");
    assert.match(source, /label=\{[^}]*itemLabel/);
});

test("the config popup labels sub-options with their names", () => {
    const source = read("src/lib/components/popups/ModConfigPopup.svelte");
    assert.match(source, /itemLabel=\{\(sub\) => sub\.Name\}/);
});

test("only options with sub-options get a dropdown", () => {
    const sub = { Name: "Low sway", Description: "", Include: ["low"] };
    assert.equal(hasSubOptionChoice({ SubOptions: [sub] }), true);
    assert.equal(hasSubOptionChoice({ SubOptions: [sub, sub, sub] }), true);
    assert.equal(hasSubOptionChoice({ SubOptions: [] }), false);
    assert.equal(hasSubOptionChoice({ SubOptions: null }), false);
    assert.equal(hasSubOptionChoice({}), false);
});
