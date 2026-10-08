import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      input: {
        widget: "index.html",
        settings: "settings.html",
      },
      output: {
        manualChunks(id) {
          if (id.includes("node_modules/pixi.js") || id.includes("node_modules/@pixi/")) {
            return "pixi-core";
          }
          if (id.includes("node_modules/pixi-live2d-display")) {
            return "live2d-vendor";
          }
        },
      },
    },
  },
});
