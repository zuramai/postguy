import { fileURLToPath } from 'node:url'

// Docus does not prefix the favicon with the base URL, so build it here (`/postguy/` in the Pages build).
const base = (process.env.NUXT_APP_BASE_URL || '/').replace(/\/$/, '')

// appConfig.header.logo.favicon is read by Docus's app.vue.
export default defineNuxtConfig({
  appConfig: { header: { logo: { favicon: `${base}/logo/postguy.svg` } } },
  extends: ['docus'],
  // Origin only: the GitHub Pages sub-path comes from NUXT_APP_BASE_URL (/postguy/ in CI).
  site: { name: 'Postguy', url: 'https://zuramai.github.io' },
  nitro: {
    // Serve the attributed originals from assets/ (no copies; ATTRIBUTION.toml covers them).
    publicAssets: [
      { dir: fileURLToPath(new URL('../assets/logo', import.meta.url)), baseURL: '/logo', maxAge: 0 },
      { dir: fileURLToPath(new URL('../assets/fonts', import.meta.url)), baseURL: '/fonts', maxAge: 0 },
    ],
    prerender: { failOnError: true },
  },
  fonts: {
    families: [
      { name: 'Poppins', src: '/fonts/Poppins-Regular.ttf', weight: 400 },
      { name: 'Poppins', src: '/fonts/Poppins-Medium.ttf', weight: 500 },
      { name: 'Poppins', src: '/fonts/Poppins-SemiBold.ttf', weight: 600 },
      // Nuxt UI's default face; never download it (AGENTS.md 1.4: Poppins is the one approved font).
      { name: 'Public Sans', provider: 'none' },
    ],
  },
  image: { provider: 'none' },  // no IPX-optimised copies: the logo is served as-is from assets/logo
  robots: { robotsTxt: false }, // the module refuses to emit robots.txt under a base URL
  mcp: { enabled: false },      // static GitHub Pages: no server routes
  ogImage: { enabled: false },  // no generated PNGs rendered with unattributed fonts
})
