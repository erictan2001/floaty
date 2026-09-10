import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    rollupOptions: {
      input: {
        widget: "index.html",
        settings: "settings.html",
      },
    },
  },
});
