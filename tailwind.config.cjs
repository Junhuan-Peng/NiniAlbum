/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./index.html', './src/**/*.{svelte,ts}'],
  theme: {
    extend: {
      colors: {
        background: '#101113',
        panel: '#181a1e',
        border: '#2a2d33',
        foreground: '#f4f4f5',
        muted: '#9ca3af',
        accent: '#d9ff5b',
        danger: '#fb7185'
      },
      fontFamily: {
        sans: ['Inter', '-apple-system', 'BlinkMacSystemFont', 'Segoe UI', 'sans-serif']
      }
    }
  },
  plugins: []
};

