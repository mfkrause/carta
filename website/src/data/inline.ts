/**
 * Renders the inline markdown the status data uses: code spans and links, nothing else.
 *
 * The gap strings in `docs/status.toml` are authored for `docs/STATUS.md`, so a bare `#anchor`
 * link points at a heading in that document. On the site those anchors live on the compatibility
 * page, so they are rewritten to resolve there.
 */
const ANCHOR_BASE = "/formats/";

export function renderInline(markdown: string): string {
  return linkSpans(codeSpans(escapeHtml(markdown)));
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function codeSpans(text: string): string {
  return text.replace(
    /`([^`]+)`/g,
    (_match, code: string) => `<code>${code}</code>`,
  );
}

function linkSpans(text: string): string {
  return text.replace(
    /\[([^\]]+)\]\(([^)\s]+)\)/g,
    (_match, label: string, href: string) =>
      `<a href="${href.startsWith("#") ? ANCHOR_BASE + href : href}">${label}</a>`,
  );
}
