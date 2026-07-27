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
        starlightSidebarTopics([
          {
            label: "Guide",
            link: "/guide/getting-started/",
            icon: "open-book",
            items: [
              {
                label: "Start here",
                items: ["guide/getting-started"],
              },
            ],
          },
          {
            label: "Formats",
            link: "/formats/",
            icon: "document",
            items: [{ label: "Overview", items: ["formats"] }],
          },
          {
            label: "CLI",
            link: "/cli/",
            icon: "seti:powershell",
            items: [{ label: "Reference", items: ["cli"] }],
          },
          {
            label: "Library",
            link: "/library/",
            icon: "seti:rust",
            items: [{ label: "Reference", items: ["library"] }],
          },
        ]),
      ],
    }),
  ],
});
