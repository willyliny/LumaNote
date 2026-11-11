import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Each screen is its own HTML entry so it can become a separate Tauri window.
// Paths are relative to the project root (where vite is run).
export default defineConfig({
  plugins: [react()],
  base: './',
  build: {
    rollupOptions: {
      input: {
        index: 'index.html', // dev launcher only
        floating: 'floating.html',
        panel: 'panel.html',
        settings: 'settings.html',
      },
    },
  },
});
