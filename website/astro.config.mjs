// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import starlightSidebarTopics from "starlight-sidebar-topics";

export default defineConfig({
  site: "https://carta.rs",
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
      customCss: ["./src/styles/theme.css"],
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
        // Each topic autogenerates from its directory, so a new page joins the sidebar by
        // existing rather than by being listed here.
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
