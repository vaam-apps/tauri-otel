import { readFileSync } from "node:fs";
import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";

// Served from GitHub Pages at /tauri-otel/. The docs workflow passes the path
// Pages reports as DOCS_BASE on main, and an empty string on pull requests,
// hence `||` rather than `??`.
const base = process.env.DOCS_BASE || "/tauri-otel/";
const repo = "https://github.com/vaam-apps/tauri-otel";

// The release these pages tell readers to pin. Read from the root
// package.json, which release-please bumps together with Cargo.toml, so the
// install snippets never fall behind a release. Pages write it as @version@
// inside code; the markdown rule below substitutes it.
const version: string = JSON.parse(
  readFileSync(new URL("../../package.json", import.meta.url), "utf8"),
).version;
const VERSION_TOKEN = /@version@/g;

export default defineMd3Config({
  base,
  lang: "en",
  title: "tauri-plugin-otel",
  titleTemplate: ":title · tauri-plugin-otel",
  description:
    "OpenTelemetry for Tauri v2: OTLP/HTTP traces and logs from the Rust side and the webview, under one resource and behind one export floor.",
  cleanUrls: true,
  lastUpdated: true,
  // A link to a page that does not exist fails the build.
  ignoreDeadLinks: false,
  head: [["link", { rel: "icon", type: "image/svg+xml", href: `${base}logo.svg` }]],

  markdown: {
    config(md) {
      md.core.ruler.push("release_version", (state) => {
        for (const token of state.tokens) {
          if (token.type === "fence" || token.type === "code_block") {
            token.content = token.content.replace(VERSION_TOKEN, version);
          }
          for (const child of token.children ?? []) {
            if (child.type === "code_inline" || child.type === "text") {
              child.content = child.content.replace(VERSION_TOKEN, version);
            }
          }
        }
      });
    },
  },

  themeConfig: {
    md3: {
      style: "expressive",
      // Tauri's cyan.
      seed: "#24c8db",
    },

    logo: "/logo.svg",

    nav: [
      { text: "Guide", link: "/guide/getting-started", activeMatch: "^/guide/" },
      { text: "Rust", link: "/reference/rust", activeMatch: "^/reference/rust" },
      { text: "JavaScript", link: "/reference/javascript", activeMatch: "^/reference/javascript" },
      {
        text: "More",
        items: [
          { text: "Changelog", link: `${repo}/blob/main/CHANGELOG.md` },
          { text: "Agent skill", link: `${repo}/blob/main/skills/tauri-otel/SKILL.md` },
          { text: "All vaam-apps docs", link: "https://vaam-apps.github.io/vitepress-md3/projects/" },
        ],
      },
    ],

    sidebar: [
      {
        text: "Guide",
        items: [
          { text: "Introduction", link: "/" },
          { text: "Getting started", link: "/guide/getting-started" },
          { text: "Configuration", link: "/guide/configuration" },
          { text: "Permissions", link: "/guide/permissions" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Rust API", link: "/reference/rust" },
          { text: "JavaScript API", link: "/reference/javascript" },
        ],
      },
      {
        text: "Concepts",
        items: [
          { text: "How it works", link: "/concepts/how-it-works" },
          { text: "Export floor and breadcrumbs", link: "/concepts/export-floor" },
          { text: "Design notes", link: "/concepts/design-notes" },
        ],
      },
      {
        text: "Help",
        items: [
          { text: "Troubleshooting", link: "/help/troubleshooting" },
          { text: "Development", link: "/help/development" },
        ],
      },
    ],

    outline: { level: [2, 3] },
    search: { provider: "local" },
    socialLinks: [{ icon: "github", link: repo }],
    editLink: {
      pattern: `${repo}/edit/main/docs/:path`,
      text: "Edit this page on GitHub",
    },
    footer: {
      message: "Released under the MIT License.",
    },
  },
});
