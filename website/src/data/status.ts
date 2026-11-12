import { z } from "astro/zod";
import { parse } from "smol-toml";
import source from "../../../docs/status.toml?raw";

export interface Family {
  key: string;
  title: string;
}

/** Format families in presentation order. */
export const families: Family[] = [
  { key: "markdown", title: "Markdown family" },
  { key: "html", title: "HTML & slides" },
  { key: "tex", title: "TeX & typesetting" },
  { key: "lightweight", title: "Lightweight markup" },
  { key: "wiki", title: "Wikis" },
  { key: "roff", title: "roff" },
  { key: "office", title: "Word processor, ebook & notebook" },
  { key: "xml", title: "XML & publishing" },
  { key: "bibliography", title: "Bibliography" },
  { key: "data", title: "Data, interchange & terminal" },
];

const support = z.enum([
  "usable",
  "in-development",
  "not-started",
  "not-applicable",
]);

export type Support = z.infer<typeof support>;

const schema = z.strictObject({
  meta: z.strictObject({
    "oracle-version": z.string(),
    "ast-version": z.string(),
  }),
  format: z.array(
    z.strictObject({
      name: z.string(),
      title: z.string(),
      family: z.enum(families.map((family) => family.key)),
      aliases: z.array(z.string()).default([]),
      read: support,
      write: support,
      "read-feature": z.string().optional(),
      "write-feature": z.string().optional(),
      "read-gaps": z.array(z.string()).default([]),
      "write-gaps": z.array(z.string()).default([]),
    }),
  ),
  extensions: z.strictObject({
    supported: z.array(z.string()),
    "recognized-not-modeled": z.array(z.string()),
    gap: z
      .array(
        z.strictObject({ names: z.array(z.string()), description: z.string() }),
      )
      .default([]),
  }),
  feature: z.array(
    z.strictObject({
      name: z.string(),
      status: support,
      notes: z.string().default(""),
    }),
  ),
});

const status = schema.parse(parse(source));

export interface DirectionStatus {
  status: Support;
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

const rank = (family: string) =>
  families.findIndex(({ key }) => key === family);

/** Every format, ordered by family and then by name. */
export const formats: Format[] = status.format
  .map((format) => ({
    name: format.name,
    title: format.title,
    family: format.family,
    aliases: format.aliases,
    read: {
      status: format.read,
      feature: format["read-feature"] ?? null,
      gaps: format["read-gaps"],
    },
    write: {
      status: format.write,
      feature: format["write-feature"] ?? null,
      gaps: format["write-gaps"],
    },
  }))
  .sort(
    (a, b) =>
      rank(a.family) - rank(b.family) ||
      Number(a.name > b.name) - Number(a.name < b.name),
  );

export const meta = {
  pandocVersion: status.meta["oracle-version"],
  astVersion: status.meta["ast-version"],
};

export const extensions = {
  supported: status.extensions.supported,
  recognizedNotModeled: status.extensions["recognized-not-modeled"],
  gaps: status.extensions.gap,
};

export const features = status.feature;

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

/** Whether a direction ships, at any maturity. */
const ships = (status: Support) =>
  status === "usable" || status === "in-development";

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

export function shipping(): Format[] {
  return formats.filter(
    (format) => ships(format.read.status) || ships(format.write.status),
  );
}

export function allNames(format: Format): string[] {
  return [format.name, ...format.aliases];
}
