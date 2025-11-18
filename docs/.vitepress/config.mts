import { defineConfig } from "vitepress";
import { tabsMarkdownPlugin } from "vitepress-plugin-tabs";

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "fht-compositor",
  description: "A dynamic tiling Wayland compositor.",
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    search: { provider: "local" },

    nav: [
      { text: "Home", link: "/" },
      { text: "Getting started", link: "/getting-started/introduction" },
      { text: "Configuration", link: "/configuration/introduction" },
    ],

    sidebar: [
      {
        text: "Getting started",
        items: [
          { text: "Introduction", link: "/getting-started/introduction" },
          { text: "Installing", link: "/getting-started/installing" },
          { text: "Guided tour", link: "/getting-started/guided-tour" },
          {
            text: "Important software",
            link: "/getting-started/important-software",
          },
          {
            text: "Example setup with Nix flakes",
            link: "/getting-started/example-nix-setup",
          },
        ],
      },

      {
        text: "Usage",
        items: [
          { text: "Workspaces", link: "/usage/workspaces" },
          { text: "Dynamic layouts", link: "/usage/layouts" },
          { text: "XWayland", link: "/usage/xwayland" },
          { text: "Nix modules", link: "/usage/nix" },
          { text: "Portals", link: "/usage/portals" },
          { text: "IPC", link: "/usage/ipc" },
        ],
      },

      {
        text: "Configuration",
        items: [
          { text: "Introduction", link: "/configuration/introduction" },
          { text: "General", link: "/configuration/general" },
          { text: "Input", link: "/configuration/input" },
          { text: "Keybindings", link: "/configuration/keybindings" },
          { text: "Mousebindings", link: "/configuration/Mousebindings" },
          { text: "Window rules", link: "/configuration/window-rules" },
          { text: "Layer rules", link: "/configuration/layer-rules" },
          { text: "Outputs", link: "/configuration/outputs" },
          { text: "Cursor theme", link: "/configuration/cursor" },
          { text: "Decorations", link: "/configuration/decorations" },
          { text: "Animations", link: "/configuration/animations" },
        ],
      },
    ],

    socialLinks: [
      { icon: "github", link: "https://github.com/nferhat/fht-compositor" },
      { icon: "discord", link: "https://discord.gg/H58C8AdU7x" },
      {
        icon: "matrix",
        link: "https://matrix.to/#/#fht-compositor:matrix.org",
      },
    ],
  },
  markdown: {
    config(md) {
      md.use(tabsMarkdownPlugin);
    },
  },
  base: "/fht-compositor/",
});
