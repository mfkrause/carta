import mdxServer from "@astrojs/mdx/server.js";
import type { APIContext, GetStaticPaths } from "astro";
import { experimental_AstroContainer } from "astro/container";
import { getCollection, render, type CollectionEntry } from "astro:content";
import type { RootContent } from "hast";
import { matches } from "hast-util-select";
import rehypeParse from "rehype-parse";
import rehypeRemark from "rehype-remark";
import remarkGfm from "remark-gfm";
import remarkStringify from "remark-stringify";
import { unified } from "unified";
import { remove } from "unist-util-remove";

/**
 * Serves every docs page as Markdown at the same path with a `.md` suffix, so an agent can read one
 * page without pulling the whole corpus. Pages are rendered the way a browser gets them and
 * converted back, which is what lets a page built from components carry its generated tables here.
 */

// Pages whose value is entirely in the browser: navigation furniture, not documentation.
const EXCLUDED = new Set(["404"]);

const container = await experimental_AstroContainer.create({
  renderers: [{ name: "astro:jsx", ssr: mdxServer }],
});

// Chrome that only means something to a pointer: the heading anchor Starlight adds, and the
// screen-reader labels that come with it.
const CHROME = ["a.sl-anchor-link", ".sr-only"];

const htmlToMarkdown = unified()
  .use(rehypeParse, { fragment: true })
  .use(function stripChrome() {
    return (tree) => {
      remove(tree, (node) =>
        CHROME.some((selector) => matches(selector, node as RootContent)),
      );
    };
  })
  .use(rehypeRemark)
  .use(remarkGfm)
  .use(remarkStringify);

/** The route a docs entry is published at, with an `index` segment dropped the way Astro drops it. */
function routeOf(entry: CollectionEntry<"docs">): string {
  return entry.id.replace(/(^|\/)index$/, "");
}

export const getStaticPaths: GetStaticPaths = async () => {
  const docs = await getCollection("docs");
  return docs
    .filter((entry) => !EXCLUDED.has(entry.id))
    .map((entry) => ({
      params: { slug: routeOf(entry) || undefined },
      props: { entry },
    }));
};

export async function GET(context: APIContext) {
  const entry = context.props.entry as CollectionEntry<"docs">;
  const { Content } = await render(entry);
  const html = await container.renderToString(Content, context);
  const body = String(await htmlToMarkdown.process(html)).trim();

  const { title, description } = entry.data;
  const heading = description ? `# ${title}\n\n> ${description}` : `# ${title}`;

  return new Response(`${heading}\n\n${body}\n`, {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
}
