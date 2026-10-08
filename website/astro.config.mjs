// @ts-check
import { defineConfig } from "astro/config";
import { satteri } from "@astrojs/markdown-satteri";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";
import starlightSidebarTopics from "starlight-sidebar-topics";

export default defineConfig({
  site: "https://carta.rs",
  vite: {
    environments: {
      prerender: {
        resolve: {
          // Keep the native loader beside its optional platform packages.
          external: ["satteri"],
        },
      },
    },
  },
  markdown: {
    // Disable smart punctuation to preserve CLI flags such as `--from`.
    processor: satteri({ features: { smartPunctuation: false } }),
  },
  integrations: [
    starlight({
      title: "carta",
      description:
        "A fast, lightweight document converter. Compatible with pandoc formats, extensions, and JSON AST.",
      logo: {
        src: "./src/assets/logo.png",
        alt: "carta",
      },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/mfkrause/carta",
        },
      ],
      editLink: {
        baseUrl: "https://github.com/mfkrause/carta/edit/main/website/",
      },
      favicon: "/favicon.png",
      customCss: ["./src/styles/theme.css"],
      components: {
        PageTitle: "./src/components/PageTitle.astro",
      },
      head: [
        {
          tag: "script",
          attrs: {
            defer: true,
            "data-domain": "carta.rs",
            src: "https://pa.kuatsu.de/js/script.js",
          },
        },
      ],
      plugins: [
        starlightLinksValidator({ errorOnRelativeLinks: false }),
        starlightLlmsTxt({
          projectName: "carta",
          description:
            "carta is a document converter written in Rust. It reads and writes the formats pandoc does, accepts the same format names and extension toggles, and speaks the same JSON AST, so it drops into an existing pipeline.",
          details: [
            "Important notes:",
            "",
            "- carta is pre-1.0. Formats differ in maturity; the compatibility page marks each reader and writer, and lists the known gaps per format.",
            "- Not supported today: PDF output, citation processing, Lua filters, `--defaults` files, and more than one input file per invocation.",
            "- Readers and writers are Cargo features. A build can carry only the directions it needs.",
          ].join("\n"),
          optionalLinks: [
            {
              label: "carta on docs.rs",
              url: "https://docs.rs/carta",
              description:
                "Full API reference for using carta as a Rust library.",
            },
            {
              label: "Source on GitHub",
              url: "https://github.com/mfkrause/carta",
              description: "Readers, writers, and the conformance suite.",
            },
          ],
          // Also remove the 404 lead from llms-full.txt; `exclude` affects only llms-small.txt.
          exclude: ["404"],
          demote: ["404"],
          customSelectors: { all: ["a.sl-anchor-link", ".sr-only"] },
        }),
        // Directory-based sidebars include new pages automatically.
        starlightSidebarTopics([
          {
            label: "Guide",
            link: "/guide/getting-started/",
            icon: "open-book",
            items: [{ autogenerate: { directory: "guide" } }],
          },
          {
            label: "Formats",
            link: "/formats/",
            icon: "document",
            items: [{ autogenerate: { directory: "formats" } }],
          },
          {
            label: "CLI",
            link: "/cli/",
            icon: "seti:powershell",
            items: [{ autogenerate: { directory: "cli" } }],
          },
          {
            label: "Library",
            link: "/library/",
            icon: "seti:rust",
            items: [{ autogenerate: { directory: "library" } }],
          },
        ]),
      ],
    }),
  ],
});
