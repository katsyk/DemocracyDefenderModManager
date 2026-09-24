import type { UUID } from "$lib/types/uuid";

/**
 * A single, provider-neutral declaration of where a mod can be found.
 * `Provider` is free-form (well-known values: "nexus", "modworkshop",
 * "github", "gamebanana", "url"), but any other site name is accepted.
 */
export type Source = {
    readonly Provider: string;
    readonly Id?: string;
    readonly Url?: string;
    readonly Version?: string;
};

export namespace legacy {
    export type Manifest = {
        readonly Guid: UUID;
        readonly Name: string;
        readonly Description: string;
        readonly IconPath?: string;
        readonly Options?: string[];
    };
}

export namespace v1 {
    export type Manifest = {
        Version: 1;
        readonly Guid: UUID;
        readonly Name: string;
        readonly Description: string;
        readonly IconPath?: string;
        readonly Options?: Option[];
        readonly NexusData?: NexusData;
        readonly Sources?: Source[];
    };

    export type Option = {
        readonly Name: string;
        readonly Description: string;
        readonly Include?: string[];
        readonly Image?: string;
        readonly SubOptions?: SubOption[];
    };

    export type SubOption = {
        readonly Name: string;
        readonly Description: string;
        readonly Include: string[];
        readonly Image?: string;
    };

    export type NexusData = {
        readonly ModId: number;
        readonly Version: string;
    };
}

export namespace v2 {
    export type Manifest = {
        Version: 2;
        readonly Guid: UUID;
        readonly Name: string;
        readonly Description: string;
        readonly IconPath?: string;
        readonly Options?: Option[];
        readonly Categories?: Category[];
        readonly Tags?: string[];
        readonly NexusData?: NexusData;
        readonly Sources?: Source[];
    };

    export type Option = {
        readonly Guid: UUID;
        readonly Name: string;
        readonly CategoryRef?: UUID;
        readonly Description: string;
        readonly Include?: string[];
        readonly Image?: string;
        readonly SubOptions?: SubOption[]; 
    };

    export type SubOption = {
        readonly Guid: UUID;
        readonly Name: string;
        readonly Description: string;
        readonly Include: string[];
        readonly Image?: string;
    };

    export type Category = {
        readonly Guid: UUID;
        readonly Name: string;
        readonly Description: string;
    };

    export type NexusData = {
        readonly ModId: number;
    };
}

export type Manifest = legacy.Manifest | v1.Manifest | v2.Manifest;