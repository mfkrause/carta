import generated from "./generated/extensions.json";

/** The number of extension names the library models, however far along each is. */
export const count: number = generated.count;
export const supported: string[] = generated.supported;
export const recognizedNotModeled: string[] = generated.recognizedNotModeled;
