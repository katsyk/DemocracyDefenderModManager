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
    const r = { logs: [], saves: [], calls: [], windowCalls: [], errors: [], results: [], expect(ok, what) { this.results.push([!!ok, what]); } };
    page.on("pageerror", e => r.errors.push(e.message));
    await page.exposeFunction("__smokeLog", (level, message) => r.logs.push({ level, message }));
    await page.exposeFunction("__smokeSave", (config) => r.saves.push(config));
    await page.exposeFunction("__smokeCall", (cmd, args) => r.calls.push({ cmd, args }));
    await page.exposeFunction("__smokeWindowCall", (cmd) => r.windowCalls.push(cmd));
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
                    (window.__smokeListeners ??= {})[args.event] = args.handler;
                    return next++;
                }
                if (!cmd.startsWith("plugin:")) window.__smokeCall(cmd, args ?? null);
                if (cmd.startsWith("plugin:window|")) window.__smokeWindowCall(cmd);
                if (cmd in (responses.__fails ?? {})) throw responses.__fails[cmd];
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
        window.__smokeEmit = (event, payload) => window[`_${window.__smokeListeners[event]}`]({ event, id: 0, payload });
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
