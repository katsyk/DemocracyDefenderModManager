import type { Component } from "svelte";
import ConfirmPopupComponent from "$lib/components/popups/ConfirmPopup.svelte";
import InputPopupComponent from "$lib/components/popups/InputPopup.svelte";
import WaitPopupComponent from "$lib/components/popups/WaitPopup.svelte";
import NotificationPopupComponent from "$lib/components/popups/NotificationPopup.svelte";
import ErrorPopupComponent from "$lib/components/popups/ErrorPopup.svelte";
import AddResultPopupComponent from "$lib/components/popups/AddResultPopup.svelte";
import ModConfigPopupComponent from "$lib/components/popups/ModConfigPopup.svelte";
import HandoffPopupComponent from "$lib/components/popups/HandoffPopup.svelte";
import BridgeConsentPopupComponent from "$lib/components/popups/BridgeConsentPopup.svelte";
import AutoImportPopupComponent from "$lib/components/popups/AutoImportPopup.svelte";
import UpdatesPopupComponent from "$lib/components/popups/UpdatesPopup.svelte";
import UpdateFilePickPopupComponent from "$lib/components/popups/UpdateFilePickPopup.svelte";
import UpdateDownloadPopupComponent from "$lib/components/popups/UpdateDownloadPopup.svelte";
import BrowserUpdatePopupComponent from "$lib/components/popups/BrowserUpdatePopup.svelte";
import type { ModAddResult } from "./results";
import type { Config } from "$lib/models/profile";
import type { Mod } from "$lib/models/mod";
import type { UUID } from "./uuid";
import type { BridgeConsentDecision, UpdateCheckReport, UpdateFile, UpdateStatusEntry } from "$lib/utils/commands";

export abstract class Popup<T = void> {
    abstract component: Component<any, any, any>;

    private _resolve!: (value: T) => void;
    readonly promise: Promise<T> = new Promise(resolve => {
        this._resolve = resolve;
    });

    close(result: T) {
        console.debug(this);
        this._resolve(result);
    }
}

export class ConfirmPopup extends Popup<boolean> {
    component = ConfirmPopupComponent;

    constructor(
        public readonly title: string,
        public readonly question: string
    ) {
        super();
    }
}

export class InputPopup extends Popup<string | null> {
    component = InputPopupComponent;

    constructor(
        public readonly placeholder: string,
        public readonly allowEmpty: boolean = false,
        public readonly minLength?: number,
        public readonly maxLength?: number,
        public readonly format?: RegExp,
        public readonly description?: string
    ) {
        super();
    }
}

export class WaitPopup extends Popup {
    component = WaitPopupComponent;

    constructor(public readonly message: string) {
        super();
    }
}

export class NotificationPopup extends Popup {
    component = NotificationPopupComponent;

    constructor(
        public readonly kind: 'info' | 'warning' | 'error',
        public readonly message: string
    ) {
        super();
    }
}

export class ErrorPopup extends Popup {
    component = ErrorPopupComponent;

    constructor(
        public readonly message: string,
        public readonly errorMessage: string
    ) {
        super();
    }
}

export class AddResultPopup extends Popup {
    component = AddResultPopupComponent;

    constructor(public readonly results: ModAddResult[]) {
        super();
    }
}

export class ModConfigPopup extends Popup<Config | null> {
    component = ModConfigPopupComponent;

    constructor(
        public readonly mod: Mod,
        public readonly config: Config
    ) {
        super();
    }
}

export type HandoffResult =
    | { status: "Done"; mod: Mod; warning?: string }
    | { status: "Cancelled" }
    | { status: "TimedOut" }
    | { status: "Error"; message: string };

/**
 * Shown while the browser handoff (see `commands::handoff` on the Rust
 * side) opens a login-gated mod page and watches the Downloads folder for
 * the archive to land.
 */
export class HandoffPopup extends Popup<HandoffResult> {
    component = HandoffPopupComponent;

    constructor(
        public readonly pageUrl: string,
        public readonly siteName: string,
        public readonly downloadsPath: string,
        public readonly existingGuid?: UUID
    ) {
        super();
    }
}

/**
 * "Allow the DDMM browser extension to install mods from **site**?" -- the
 * per-site consent prompt the bridge asks the first time a site tries to
 * install through the extension. See `docs/development/bridge-protocol.md`.
 */
export class BridgeConsentPopup extends Popup<BridgeConsentDecision> {
    component = BridgeConsentPopupComponent;

    constructor(public readonly site: string) {
        super();
    }
}

export type AutoImportDecision = "Install" | "InstallAndDeploy" | "Ignore";

/**
 * A new archive appeared in the Downloads folder while
 * [auto-import](../../../docs/using/one-click-install.md) was on and
 * looked like a Helldivers 2 mod -- asks what to do with it. Never
 * triggered without this confirmation, however it looks; "auto" only
 * means "auto-detected", not "auto-installed".
 */
export class AutoImportPopup extends Popup<AutoImportDecision> {
    component = AutoImportPopupComponent;

    constructor(public readonly file: string) {
        super();
    }
}
export type UpdatesPopupAction =
    | { kind: "update"; entry: UpdateStatusEntry }
    | { kind: "updateAll" }
    | { kind: "skip"; entry: UpdateStatusEntry }
    | { kind: "unskip"; entry: UpdateStatusEntry }
    | { kind: "nexusSettings" }
    | null;

/**
 * The results of a "Check for Updates" click: every checked mod/source with
 * its state, per-mod Update / Skip this version, "Update all", and a link to
 * the optional Nexus API key setting when Nexus mods need one.
 */
export class UpdatesPopup extends Popup<UpdatesPopupAction> {
    component = UpdatesPopupComponent;

    constructor(
        public readonly report: UpdateCheckReport,
        public readonly modNames: Map<UUID, string>
    ) {
        super();
    }
}

/** Several files could be the update (multi-file mods): ask which one. */
export class UpdateFilePickPopup extends Popup<UpdateFile | null> {
    component = UpdateFilePickPopupComponent;

    constructor(
        public readonly modName: string,
        public readonly entry: UpdateStatusEntry
    ) {
        super();
    }
}

export type UpdateDownloadResult =
    | { ok: true; mod: Mod; warning?: string }
    | { ok: false; message: string };

/** Downloads and installs a one-click (direct) update, with progress. */
export class UpdateDownloadPopup extends Popup<UpdateDownloadResult> {
    component = UpdateDownloadPopupComponent;
    /** Set once the download has been started, so re-mounting the popup
     * (another popup shown on top, then closed) never starts it twice. */
    started = false;

    constructor(
        public readonly modName: string,
        public readonly entry: UpdateStatusEntry,
        public readonly file: UpdateFile,
        public readonly position?: { index: number; total: number }
    ) {
        super();
    }
}

export type BrowserUpdateDecision = "Done" | "Handoff" | "Skip" | "Stop";

/**
 * An update that needs the browser (login-gated site such as AyakaMods or
 * Nexus Mods). With the extension connected, DDMM opens the mod page and
 * the extension's "Update with DDMM" button finishes it; otherwise (or on
 * request) it falls back to the Downloads-folder handoff.
 */
export class BrowserUpdatePopup extends Popup<BrowserUpdateDecision> {
    component = BrowserUpdatePopupComponent;

    constructor(
        public readonly modName: string,
        public readonly entry: UpdateStatusEntry,
        public readonly extensionActive: boolean,
        public readonly position?: { index: number; total: number }
    ) {
        super();
    }
}
