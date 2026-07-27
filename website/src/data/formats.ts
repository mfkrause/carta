import generated from "./generated/formats.json";

export type Support =
  "usable" | "in-development" | "not-started" | "not-applicable";

export interface DirectionStatus {
  status: Support;
  ships: boolean;
  feature: string | null;
  gaps: string[];
}

export interface Format {
  name: string;
  title: string;
  family: string;
  aliases: string[];
  read: DirectionStatus;
  write: DirectionStatus;
}

export interface Family {
  key: string;
  title: string;
}

export const families: Family[] = generated.families;
export const formats: Format[] = generated.formats as Format[];

export const SUPPORT_LABEL: Record<Support, string> = {
  usable: "Usable",
  "in-development": "In development",
  "not-started": "Not started",
  "not-applicable": "Not applicable",
};

export const SUPPORT_MARKER: Record<Support, string> = {
  usable: "✅",
  "in-development": "🚧",
  "not-started": "❌",
  "not-applicable": "➖",
};

/** Formats grouped in the family order the status data declares, empty families dropped. */
export function byFamily(
  subset: Format[] = formats,
): { family: Family; formats: Format[] }[] {
  return families
    .map((family) => ({
      family,
      formats: subset.filter((format) => format.family === family.key),
    }))
    .filter((group) => group.formats.length > 0);
}

/** Formats carta can actually convert, in either direction. */
export function shipping(): Format[] {
  return formats.filter((format) => format.read.ships || format.write.ships);
}

export function findFormat(name: string): Format | undefined {
  return formats.find(
    (format) => format.name === name || format.aliases.includes(name),
  );
}

/** Every name a format answers to, canonical first. */
export function allNames(format: Format): string[] {
  return [format.name, ...format.aliases];
}
