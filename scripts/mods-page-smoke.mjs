// Headless smoke test of the built Mods page against a mocked backend.
//
//   pnpm build
//   node scripts/mods-page-smoke.mjs [scenario...]
//
// Needs `playwright-core` (resolved normally, or from the folder named by
// PLAYWRIGHT_CORE) and a Chromium (CHROME, or Playwright's own download).
// Not part of CI: it's for checking a fix of the Mods page in a real browser
// engine without a desktop build. Each scenario serves `build/`, replaces
// the Tauri IPC with canned data (mods with V1, V2 and legacy manifests,
// options with empty or null sub-options, a "Mod not found" entry, an
// update report) and checks what the page shows.
import { createRequire } from "node:module";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE ?? "playwright-core");
const buildDir = path.resolve(process.env.BUILD_DIR ?? "build");

const G = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

function fixture() {
    const mods = [
        { Manifest: { Version: 1, Guid: G(1), Name: "V1 mod", Description: "v1", IconPath: "icon.png",
            Options: [{ Name: "A", Description: "", SubOptions: [] }, { Name: "B", Description: "", SubOptions: null },
                { Name: "C", Description: "", SubOptions: [{ Name: "c1", Description: "", Include: [] }, { Name: "c2", Description: "", Include: [] }] }] },
          Directory: "/data/mods/V1 mod 1.0.0 2026-09-27T03-15Z", Sources: [{ Provider: "nexus", DisplayName: "Nexus Mods", PageUrl: "https://example.invalid/1", Origin: "Manifest" }] },
        { Manifest: { Version: 2, Guid: G(2), Name: "First Person (experimental)", Description: "v2", Tags: ["camera"], Options: [] }, Directory: "/data/mods/v2", Sources: [] },
        { Manifest: { Guid: G(3), Name: "Legacy mod", Description: "legacy", Options: ["one", "two"] }, Directory: "/data/mods/legacy" },
        { Manifest: { Version: 1, Guid: G(4), Name: "Leniently parsed mod", Description: "trailing comma", Options: null }, Directory: "/data/mods/lenient" },
    ];
    const configs = [
        { For: "V1", Guid: G(1), Enabled: true, Toggled: [true, true, true], Selected: [0, 0, 1] },
        { For: "V2", Guid: G(2), Enabled: true, Toggled: [], Selected: [] },
        { For: "Legacy", Guid: G(3), Enabled: false, Selected: 1 },
        { For: "V1", Guid: G(4), Enabled: true, Toggled: [], Selected: [] },
        { For: "V1", Guid: G(99), Enabled: true, Toggled: [], Selected: [] },
    ];
    const report = { Results: [
        { Guid: G(1), Provider: "nexus", DisplayName: "Nexus Mods", Method: "Browser", Status: { Kind: "UpdateAvailable" }, LatestVersion: "2.0" },
        { Guid: G(2), Provider: "ayakamods", DisplayName: "AyakaMods", Method: "Direct", Files: [], Status: { Kind: "NeedsManualCheck" } },
    ] };
    return {
        get_mods: mods,
        load_profiles: { Profiles: [{ Version: "V1", Name: "Default", Configs: configs }, { Version: "V1", Name: "Second", Configs: [] }], Active: 0 },
        load_settings: { Version: "V1", GamePath: "/game", SkipList: [], DownloadsPath: "/dl", AfterBrowserInstall: "deploy", BridgeAllowedSites: [], AutoImportEnabled: false, AutoCheckUpdates: false, AutoCheckIntervalHours: 0 },
        check_settings: true,
        get_last_update_report: report,
        get_data_folder_info: { Path: "/data", Problem: null },
        resolve_mod_image: null,
        take_pending_deep_links: [],
        detect_import_sources: [],
        // Settings
        get_data_dir: "/data",
        validate_game_path: { valid: true, resolvedPath: null, code: null, detail: null, message: null },
        get_nexus_key_status: { Present: false },
        get_nexus_sign_in_status: { Available: false, SignedIn: false, Port: 28647 },
        repair_browser_integration: [],
    };
}

const rows = (page) => page.evaluate(() => [...document.querySelectorAll("main span.text-2xl")].map(s => s.textContent.trim()));

const scenarios = {
    // Everything renders.
    async base(page, r) {
        r.expect((await rows(page)).includes("First Person (experimental)"), "the mod list is shown");
        r.expect(await page.getByTestId("missing-mod-entry").count() === 1, "the missing entry is shown");
    },
    // A mod listed twice in profiles.json: once an endless "Loading...".
    dup: {
        data: (d) => d.load_profiles.Profiles[0].Configs.push({ For: "V2", Guid: G(2), Enabled: false, Toggled: [], Selected: [] }),
        async check(page, r) {
            r.expect((await rows(page)).includes("First Person (experimental)"), "the mod list is shown");
            const saved = r.saves.at(-1);
            r.expect(saved && saved.Profiles[0].Configs.filter(c => c.Guid === G(2)).length === 1, "the repaired profiles are saved");
        },
    },
    // Search, then edit the options of the one mod shown.
    async searchedit(page, r) {
        await page.locator("main input[type=text]").first().fill("First Person (experimental)");
        await page.locator("main div.rounded button.hd2mm-button-nop.p-2:not(.invisible)").first().click({ timeout: 2000 });
        await page.locator("button.hd2mm-button.self-end").last().click({ timeout: 2000 });
        await page.locator("main input[type=text]").first().fill("");
        await page.waitForTimeout(300);
        const shown = await rows(page);
        r.expect(shown.filter(n => n === "First Person (experimental)").length === 1 && shown.includes("V1 mod"), "no entry was overwritten");
    },
    // Something the list can't render: an error screen, not a spinner.
    badreport: {
        data: (d) => d.get_last_update_report.Results.push({ Guid: G(2), Provider: "x", DisplayName: "X", Method: "Browser", Status: null }),
        async check(page, r) {
            r.expect(await page.getByTestId("mods-load-error").count() === 1, "the error screen is shown");
            r.expect(r.logs.some(l => l.level === 5 && l.message.includes("couldn't be shown")), "the error is logged at ERROR");
            // Reload keeps the session's changes and lets go of the bridge first.
            const saves = r.saves.length;
            await page.getByTestId("mods-reload").click({ timeout: 2000 });
            await page.waitForTimeout(1500);
            r.expect(r.saves.length > saves, "Reload saves the profiles first");
            r.expect(r.calls.some(c => c.cmd === "set_bridge_frontend_ready" && c.args?.ready === false), "Reload marks the bridge frontend not ready");
        },
    },
    // A mod with two pages on the same site (two Nexus IDs): two rows in
    // the update check's popup, not a "Checking for updates..." that never
    // ends.
    updatekeys: {
        data: (d) => {
            const e = (id) => ({ Guid: G(1), Provider: "nexus", DisplayName: "Nexus Mods", SourceId: id, InstalledVersion: "1.0", LatestVersion: "1.0", Method: "Browser", Status: { Kind: "UpToDate" } });
            d.check_updates = { Trigger: "Manual", Results: [e("100"), e("200")] };
        },
        async check(page, r) {
            await page.locator("button[title^='Check installed mods']").click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(await page.getByText("Checking for updates...").count() === 0, "the check doesn't hang");
            r.expect(await page.locator("li", { hasText: "Nexus Mods" }).count() === 2, "both sources are listed");
            r.expect(r.errors.length === 0, "no page error");
        },
    },
    // A popup that throws while rendering: an error with Close, which
    // closes it, instead of a popup (and its caller) stuck for good.
    popuperror: {
        data: (d) => {
            d.check_updates = { Trigger: "Manual", Results: [{ Guid: G(1), Provider: "nexus", DisplayName: "Nexus Mods", SourceId: "1", Method: "Browser", Status: { Kind: "UpToDate" } }] };
            d.__poisonInstalledVersion = true;
        },
        async check(page, r) {
            await page.locator("button[title^='Check installed mods']").click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(await page.getByTestId("popup-render-error").count() === 1, "the popup shows its error");
            r.expect(r.logs.some(l => l.level === 5 && l.message.includes("popup couldn't be shown")), "the error is logged at ERROR");
            await page.getByText("Close", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(300);
            r.expect(await page.getByTestId("popup-render-error").count() === 0, "Close closes it");
            r.expect((await rows(page)).includes("V1 mod"), "the Mods page is still there");
        },
    },
    // Enter or Space while a popup is shown: never presses the page's
    // button that opened it again (a second deploy behind the first).
    async keysbehind(page, r) {
        await page.locator("button.hd2mm-success-button", { hasText: "Deploy" }).click({ timeout: 2000 });
        await page.waitForTimeout(400);
        await page.keyboard.press("Enter");
        await page.waitForTimeout(400);
        await page.keyboard.press("Space");
        await page.waitForTimeout(400);
        r.expect(r.calls.filter(c => c.cmd === "deploy").length === 1, "Deploy ran once");
    },
    // Delete in the library, and a browser install lands while "Are you
    // sure?" is shown: the mod clicked is deleted, not the one now in its
    // place.
    deleterace: {
        data: (d) => d.load_profiles.Profiles[0].Configs.splice(1, 1),
        async check(page, r) {
            await page.locator("main button[title='Library']").click({ timeout: 2000 });
            await page.waitForTimeout(500);
            await page.locator("main li button[title='Delete']").first().click({ timeout: 2000 });
            await page.waitForTimeout(300);
            await page.evaluate((g) => {
                const r = window.__smokeResponses;
                r.get_mods.unshift({ Manifest: { Version: 1, Guid: g, Name: "Browser mod", Description: "", Options: [] }, Directory: "/data/mods/b" });
                window.__smokeEmit("bridge://mod-installed", { requestId: "1", mod: { guid: g, name: "Browser mod" }, afterInstall: "library" });
            }, G(5));
            await page.waitForTimeout(500);
            await page.getByText("Yes", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            const deleted = r.calls.filter(c => c.cmd === "delete_mod").map(c => c.args.guid);
            r.expect(deleted.length === 1 && deleted[0] === G(2), `the mod clicked is deleted (deleted ${deleted.join(", ")})`);
        },
    },
    // Leaving Settings when the backend refuses to save them: the reason
    // is shown, not swallowed.
    settingssavefail: {
        data: (d) => { d.__fails = { save_settings: "The downloads folder is DDMM's own mod storage." }; },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.locator("a[href='/']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            r.expect(r.calls.some(c => c.cmd === "save_settings"), "Settings tried to save");
            r.expect(await page.getByText("DDMM's own mod storage").count() === 1, "the reason is shown");
            r.expect(!r.errors.some(e => e.includes("mod storage")), "no unhandled rejection");
        },
    },
    // Settings that can't be loaded (a broken settings.json): the page says
    // so, and doesn't keep the user from leaving or save over the file.
    settingsloadfail: {
        data: (d) => { d.__fails = { load_settings: "settings.json isn't valid settings JSON" }; },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.locator("a[href='/help']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            r.expect(page.url().endsWith("/help"), "the user can leave Settings");
            r.expect(!r.calls.some(c => c.cmd === "save_settings"), "nothing is saved over the file");
        },
    },
    // Closing DDMM from Settings saves them first.
    settingsclose: {
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            const saves = r.calls.filter(c => c.cmd === "save_settings").length;
            await page.evaluate(() => window.__smokeEmit("tauri://close-requested", null));
            await page.waitForTimeout(800);
            r.expect(r.calls.filter(c => c.cmd === "save_settings").length === saves + 1, "the settings are saved on close");
            r.expect(r.calls.some(c => c.cmd === "ack_close_requested"), "the close is acknowledged");
            r.expect(r.windowCalls.some(c => c.includes("destroy")), "the window is closed");
        },
    },
    // A browser handoff with another popup shown on top of it for a while
    // (here the extension asking to install): the page is opened and the
    // handoff started once, not again when the handoff shows again.
    handoffcover: {
        data: (d) => { d.classify_download_url = { Provider: "nexus", DisplayName: "Nexus Mods", RequiresHandoff: true }; },
        async check(page, r) {
            await page.locator("button", { hasText: "Add URL" }).click({ timeout: 2000 });
            await page.locator("#input").fill("https://www.nexusmods.com/helldivers2/mods/1");
            await page.getByText("Confirm", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            await page.evaluate(() => window.__smokeEmit("bridge://consent-request", { requestId: "1", site: "example.com" }));
            await page.waitForTimeout(300);
            await page.getByText("Deny", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(r.calls.filter(c => c.cmd === "start_handoff").length === 1, "the handoff is started once");
            r.expect(r.pluginCalls.filter(c => c === "plugin:opener|open_url").length === 1, "the page is opened once");
            // The download lands while another popup covers the handoff.
            await page.evaluate(() => window.__smokeEmit("bridge://consent-request", { requestId: "2", site: "example.com" }));
            await page.waitForTimeout(300);
            await page.evaluate((g) => window.__smokeEmit("handoff", { Status: "Done", Mod: { Manifest: { Version: 1, Guid: g, Name: "Handed-off mod", Description: "", Options: [] }, Directory: "/data/mods/h" } }), G(6));
            await page.waitForTimeout(300);
            await page.getByText("Deny", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(await page.getByText("Waiting for the download", { exact: false }).count() === 0
                && await page.locator("button", { hasText: "Choose File" }).count() === 0, "the handoff popup is closed");
        },
    },
    movingclose: {
        data: (d) => {
            d["plugin:dialog|open"] = "/newdata";
            d.plan_data_folder_move = { Source: "/data", Picked: "/newdata", Target: "/newdata", IsReset: false, UsedSubfolder: false, ExistingData: false, TotalBytes: 1000, TotalFiles: 3, FreeBytes: null };
            d.__hang = ["move_data_folder"];
        },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.getByText("Change...", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            await page.getByText("Move", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(r.calls.some(c => c.cmd === "move_data_folder"), "the move is running");
            await page.evaluate(() => window.__smokeEmit("tauri://close-requested", null));
            await page.waitForTimeout(800);
            r.expect(!r.windowCalls.some(c => c.includes("destroy")), "the window is NOT destroyed mid-move (got " + r.windowCalls.join(",") + ")");
        },
    },
    handofferrcover: {
        data: (d) => { d.classify_download_url = { Provider: "nexus", DisplayName: "Nexus Mods", RequiresHandoff: true }; d.__fails = { start_handoff: "Downloads folder missing" }; },
        async check(page, r) {
            await page.locator("button", { hasText: "Add URL" }).click({ timeout: 2000 });
            await page.locator("#input").fill("https://www.nexusmods.com/helldivers2/mods/1");
            await page.getByText("Confirm", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(await page.getByText("Downloads folder missing").count() === 1, "error shown first");
            await page.evaluate(() => window.__smokeEmit("bridge://consent-request", { requestId: "1", site: "example.com" }));
            await page.waitForTimeout(300);
            await page.getByText("Deny", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(await page.getByText("Downloads folder missing").count() === 1, "error still shown after cover");
        },
    },
    // Closing from Settings with an invalid game path: asks first, and
    // closes without saving only when told to.
    invalidclose: {
        data: (d) => { d.validate_game_path = { valid: false, resolvedPath: null, code: "not_found", detail: null, message: null }; },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.evaluate(() => window.__smokeEmit("tauri://close-requested", null));
            await page.waitForTimeout(500);
            r.expect(!r.windowCalls.some(c => c.includes("destroy")), "not closed without asking");
            await page.getByText("Yes", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(r.windowCalls.some(c => c.includes("destroy")), "closed once confirmed");
            r.expect(!r.calls.some(c => c.cmd === "save_settings"), "the invalid settings aren't saved");
        },
    },
    // "Choose File" in the browser handoff cancels the handoff, whose
    // "Cancelled" mustn't close the popup while the chosen file installs.
    handoffchoose: {
        data: (d) => {
            d.classify_download_url = { Provider: "nexus", DisplayName: "Nexus Mods", RequiresHandoff: true };
            d["plugin:dialog|open"] = "/dl/mod.zip";
            d.__hang = ["install_handoff_file"];
        },
        async check(page, r) {
            await page.locator("button", { hasText: "Add URL" }).click({ timeout: 2000 });
            await page.locator("#input").fill("https://www.nexusmods.com/helldivers2/mods/1");
            await page.getByText("Confirm", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            await page.locator("button", { hasText: "Choose File" }).click({ timeout: 2000 });
            await page.waitForTimeout(300);
            await page.evaluate(() => window.__smokeEmit("handoff", { Status: "Cancelled" }));
            await page.waitForTimeout(300);
            r.expect(r.calls.some(c => c.cmd === "install_handoff_file"), "the chosen file is installing");
            r.expect(await page.locator("button", { hasText: "Choose File" }).count() === 1, "the popup stays open while it installs");
        },
    },
    // A settings save that never returns doesn't hold up the later ones
    // (here: the save before moving the data folder).
    savehang: {
        data: (d) => {
            d.__hangFirst = ["save_settings"];
            d["plugin:dialog|open"] = "/newdata";
            d.plan_data_folder_move = { Source: "/data", Picked: "/newdata", Target: "/newdata", IsReset: false, UsedSubfolder: false, ExistingData: false, TotalBytes: 1000, TotalFiles: 3, FreeBytes: null };
        },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.evaluate(() => window.__smokeEmit("tauri://close-requested", null));
            await page.waitForTimeout(5500);
            await page.getByText("Yes", { exact: true }).click({ timeout: 2000 }).catch(() => {});
            await page.waitForTimeout(300);
            const before = r.calls.filter(c => c.cmd === "save_settings").length;
            await page.getByText("Change...", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(6000);
            r.expect(r.calls.filter(c => c.cmd === "save_settings").length === before + 1, "the next save runs");
            r.expect(r.calls.some(c => c.cmd === "plan_data_folder_move"), "the move goes on to its plan");
        },
    },
    // "Use existing data": closing waits while the folder is switched.
    adoptclose: {
        data: (d) => {
            d["plugin:dialog|open"] = "/newdata";
            d.plan_data_folder_move = { Source: "/data", Picked: "/newdata", Target: "/newdata", IsReset: false, UsedSubfolder: false, ExistingData: true, TotalBytes: 1000, TotalFiles: 3, FreeBytes: null };
            d.__hang = ["adopt_data_folder"];
        },
        async check(page, r) {
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            await page.getByText("Change...", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            await page.getByText("Use the Existing Data", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(500);
            r.expect(r.calls.some(c => c.cmd === "adopt_data_folder"), "the switch is running");
            await page.evaluate(() => window.__smokeEmit("tauri://close-requested", null));
            await page.waitForTimeout(800);
            r.expect(!r.windowCalls.some(c => c.includes("destroy")), "the window is not closed meanwhile");
        },
    },
    // An update gave the legacy mod (installed without its manifest.json)
    // its author's ID: its entry moves to the new ID, keeping its place and
    // on/off, with the options reset to fit; an old ID that's still a
    // loaded mod's is left alone.
    rename: {
        async check(page, r) {
            await page.evaluate(([oldGuid, newGuid, inUse]) => {
                const res = window.__smokeResponses;
                const i = res.get_mods.findIndex(m => m.Manifest.Guid === oldGuid);
                res.get_mods[i] = { Manifest: { Version: 1, Guid: newGuid, Name: "Legacy mod", Description: "author's",
                    Options: [{ Name: "Red", Description: "", Include: ["Red"] }, { Name: "Blue", Description: "", Include: ["Blue"] }] },
                    Directory: "/data/mods/legacy", Sources: [] };
                res.get_guid_renames = [[oldGuid, newGuid], [inUse, newGuid]];
                window.__smokeEmit("bridge://mod-installed", { requestId: "r1", mod: { guid: newGuid, name: "Legacy mod" }, afterInstall: "library" });
            }, [G(3), G(50), G(1)]);
            await page.waitForTimeout(800);
            r.expect(r.calls.some(c => c.cmd === "get_guid_renames"), "the page asks for the renames");
            r.expect(await page.getByTestId("missing-mod-entry").count() === 1, "only the entry that was missing before is missing");
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            const configs = r.saves.at(-1)?.Profiles[0].Configs ?? [];
            const moved = configs[2];
            r.expect(configs.length === 5 && !configs.some(c => c.Guid === G(3)), `no entry keeps the old ID (${configs.map(c => c.Guid).join(", ")})`);
            r.expect(moved?.Guid === G(50) && moved.Enabled === false, "the entry keeps its place and stays off");
            r.expect(moved?.For === "V1" && JSON.stringify(moved.Toggled) === "[true,true]" && JSON.stringify(moved.Selected) === "[0,0]", `its options fit the author's (${JSON.stringify(moved)}; ${r.logs.filter(l => /new ID/.test(l.message)).map(l => l.message).join(" / ")})`);
            r.expect(configs[0].Guid === G(1), "an old ID that's still loaded isn't renamed");
        },
    },
    // An update changed a mod's options under its profile entry: Deploy
    // resets its choices to fit, and that's what is saved afterwards.
    deployfit: {
        async check(page, r) {
            await page.evaluate((guid) => {
                const res = window.__smokeResponses;
                const mod = res.get_mods.find(m => m.Manifest.Guid === guid);
                mod.Manifest.Options = mod.Manifest.Options.slice(0, 2);
                window.__smokeEmit("bridge://mod-installed", { requestId: "r1", mod: { guid, name: "V1 mod" }, afterInstall: "library" });
            }, G(1));
            await page.waitForTimeout(800);
            // Waits for the "installed" toast over the button to go.
            await page.locator("button.hd2mm-success-button", { hasText: "Deploy" }).click({ timeout: 10000 });
            await page.waitForTimeout(800);
            r.expect(r.calls.some(c => c.cmd === "deploy"), "Deploy ran");
            // "Deployed, but 1 missing mod(s) were skipped".
            await page.getByText("OK", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(400);
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            const entry = r.saves.at(-1)?.Profiles[0].Configs.find(c => c.Guid === G(1));
            r.expect(JSON.stringify(entry?.Toggled) === "[true,true]", `the fitted options are saved (${JSON.stringify(entry)})`);
        },
    },
    // Choices that don't fit the mod any more are reset when the page
    // loads, and the reset ones are what's saved.
    loadfit: {
        data: (d) => { d.load_profiles.Profiles[0].Configs[0] = { For: "V1", Guid: G(1), Enabled: false, Toggled: [true, false], Selected: [0, 0] }; },
        async check(page, r) {
            r.expect(r.logs.some(l => l.message.includes("Options reset to defaults")), "the reset is logged");
            await page.getByText("OK", { exact: true }).click({ timeout: 2000 });
            await page.waitForTimeout(400);
            await page.locator("a[href='/settings']").click({ timeout: 2000 });
            await page.waitForTimeout(800);
            const entry = r.saves.at(-1)?.Profiles[0].Configs.find(c => c.Guid === G(1));
            r.expect(JSON.stringify(entry?.Toggled) === "[true,true,true]" && entry?.Enabled === false, `the fitted options are saved (${JSON.stringify(entry)})`);
        },
    },
    // Init itself fails.
    initfail: {
        data: (d) => { d.load_profiles.Profiles = null; },
        async check(page, r) {
            r.expect(await page.getByTestId("mods-load-error").count() === 1, "the error screen is shown");
            r.expect(r.logs.some(l => l.level === 5 && l.message.includes("couldn't load")), "the error is logged at ERROR");
        },
    },
};

const server = http.createServer((req, res) => {
    const p = decodeURIComponent(new URL(req.url, "http://x").pathname);
    let f = path.join(buildDir, p);
    if (!f.startsWith(buildDir) || !fs.existsSync(f) || fs.statSync(f).isDirectory()) f = path.join(buildDir, "index.html");
    const type = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".png": "image/png" }[path.extname(f)] ?? "application/octet-stream";
    res.writeHead(200, { "content-type": type });
    fs.createReadStream(f).pipe(res);
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const browser = await chromium.launch({ executablePath: process.env.CHROME || undefined });

let failed = 0;
for (const name of process.argv.length > 2 ? process.argv.slice(2) : Object.keys(scenarios)) {
    const s = typeof scenarios[name] === "function" ? { check: scenarios[name] } : scenarios[name];
    if (!s) throw new Error(`unknown scenario ${name}`);
    const data = fixture();
    s.data?.(data);
    const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
    const r = { logs: [], saves: [], calls: [], windowCalls: [], pluginCalls: [], errors: [], results: [], expect(ok, what) { this.results.push([!!ok, what]); } };
    page.on("pageerror", e => r.errors.push(e.message));
    await page.exposeFunction("__smokeLog", (level, message) => r.logs.push({ level, message }));
    await page.exposeFunction("__smokeSave", (config) => r.saves.push(config));
    await page.exposeFunction("__smokeCall", (cmd, args) => r.calls.push({ cmd, args }));
    await page.exposeFunction("__smokeWindowCall", (cmd) => r.windowCalls.push(cmd));
    await page.exposeFunction("__smokePluginCall", (cmd) => r.pluginCalls.push(cmd));
    await page.addInitScript((responses) => {
        let next = 1;
        window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
        window.__TAURI_INTERNALS__ = {
            metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
            transformCallback(f) { const id = next++; window[`_${id}`] = f; return id; },
            unregisterCallback() {},
            convertFileSrc: (p, proto = "asset") => `${proto}://localhost/${encodeURIComponent(p)}`,
            async invoke(cmd, args) {
                if (cmd === "plugin:log|log") return void window.__smokeLog(args.level, args.message);
                if (cmd.startsWith("plugin:event|listen")) {
                    const id = next++;
                    (window.__smokeListeners ??= []).push({ id, event: args.event, handler: args.handler });
                    return id;
                }
                if (cmd === "plugin:event|unlisten") {
                    window.__smokeListeners = (window.__smokeListeners ?? []).filter(l => l.id !== args.eventId);
                    return;
                }
                if (!cmd.startsWith("plugin:")) window.__smokeCall(cmd, args ?? null);
                if (cmd.startsWith("plugin:window|")) window.__smokeWindowCall(cmd);
                if (cmd.startsWith("plugin:") && !cmd.startsWith("plugin:log|")) window.__smokePluginCall(cmd);
                if (cmd in (responses.__fails ?? {})) throw responses.__fails[cmd];
                if ((responses.__hang ?? []).includes(cmd)) return new Promise(() => {});
                if ((responses.__hangFirst ?? []).includes(cmd)) {
                    responses.__hangFirst = responses.__hangFirst.filter(c => c !== cmd);
                    return new Promise(() => {});
                }
                if (cmd === "save_profiles") return void window.__smokeSave(JSON.parse(JSON.stringify(args.config)));
                if (!(cmd in responses)) return null;
                const result = structuredClone(responses[cmd]);
                if (cmd === "check_updates" && responses.__poisonInstalledVersion) {
                    // Only the update popup reads this.
                    Object.defineProperty(result.Results[0], "InstalledVersion", { get() { throw new Error("poisoned InstalledVersion"); } });
                }
                return result;
            },
        };
        window.__smokeResponses = responses;
        // Doesn't wait for the handler (it may be waiting for a popup).
        window.__smokeEmit = (event, payload) => {
            for (const l of (window.__smokeListeners ?? []).filter(l => l.event === event)) window[`_${l.handler}`]({ event, id: l.id, payload });
        };
    }, data);
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    await page.waitForTimeout(1500);
    r.expect(await page.getByText("Loading...", { exact: true }).count() === 0, "not stuck on Loading...");
    try {
        await s.check(page, r);
    } catch (e) {
        r.expect(false, `scenario threw: ${e.message.split("\n")[0]}`);
    }
    await page.close();
    const bad = r.results.filter(([ok]) => !ok);
    failed += bad.length;
    console.log(`${bad.length ? "FAIL" : "ok  "} ${name}`);
    for (const [ok, what] of r.results) if (!ok) console.log(`       - ${what}`);
    if (bad.length) for (const e of r.errors) console.log(`       page error: ${e}`);
}

await browser.close();
server.close();
process.exit(failed ? 1 : 0);
