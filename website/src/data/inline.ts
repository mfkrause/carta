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

/**
 * The destinations the status data is allowed to link to: the compatibility page's own anchors,
 * site-relative paths, and plain web addresses. Anything else (`javascript:` above all) keeps its
 * brackets and reaches the page as text, because the result goes out through `set:html`.
 */
const SAFE_HREF = /^(#|\/|https?:\/\/)/;

function linkSpans(text: string): string {
  return text.replace(
    /\[([^\]]+)\]\(([^)\s]+)\)/g,
    (match, label: string, href: string) => {
      if (!SAFE_HREF.test(href)) return match;
      return `<a href="${href.startsWith("#") ? ANCHOR_BASE + href : href}">${label}</a>`;
    },
  );
}
