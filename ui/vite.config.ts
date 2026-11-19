import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// В режиме разработки панель живёт на Vite (горячая перезагрузка),
// а API проксируется в запущенное ядро.
const core = "http://127.0.0.1:7272";

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: core, ws: true },
      "/auth": core,
    },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    // Панель и виджеты для OBS — отдельные страницы.
    rollupOptions: {
      input: {
        panel: "index.html",
        player: "widgets/player.html",
      },
    },
  },
});
