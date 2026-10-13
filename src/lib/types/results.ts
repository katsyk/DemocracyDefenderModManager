export type RustResult<T, E = string> =
    | { Ok: T }
    | { Err: E };

export type ModAddResult =
    /** `replaced`: it replaced the installed mod with its ID; `kept`: the
     * user kept that mod instead, and nothing was installed. */
    | { success: true; archiveFile: string; warning?: string; replaced?: boolean; kept?: boolean }
    | { success: false; archiveFile: string; errorMessage: string };