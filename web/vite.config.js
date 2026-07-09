import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Standalone React + Three.js app. Kept out of the Rust workspace on purpose
// (see docs/FRONTEND_SOLARSYSTEM_SPEC.md). Astro can later wrap this scene as an
// island for SSR content/SEO; for the realism prototype a plain Vite SPA is the
// most direct thing to run and iterate on.
export default defineConfig({
  plugins: [react()],
  server: { host: true },
});
