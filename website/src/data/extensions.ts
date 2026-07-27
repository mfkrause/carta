import generated from "./generated/extensions.json";

export interface ExtensionGap {
  names: string[];
  description: string;
}

export interface FormatExtensions {
  /** Every extension name the format accepts in a `+ext`/`-ext` toggle. */
  accepted: string[];
  /** The subset the format's defaults turn on. */
  enabled: string[];
}

/** The number of extension names the library models, however far along each is. */
export const count: number = generated.count;
export const supported: string[] = generated.supported;
export const recognizedNotModeled: string[] = generated.recognizedNotModeled;
export const gaps: ExtensionGap[] = generated.gaps;

const byFormat = generated.byFormat as Record<string, FormatExtensions>;

export function extensionsFor(format: string): FormatExtensions | undefined {
  // Own properties only: a plain object still answers to "toString" and friends.
  return Object.hasOwn(byFormat, format) ? byFormat[format] : undefined;
}

export function isSupported(extension: string): boolean {
  return supported.includes(extension);
}

/** The tracked divergences that mention `extension`. */
export function gapsFor(extension: string): ExtensionGap[] {
  return gaps.filter((gap) => gap.names.includes(extension));
}
