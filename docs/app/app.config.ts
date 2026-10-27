export default defineAppConfig({
  seo: { title: 'Postguy', description: 'Offline, native API client for HTTP, gRPC and WebSocket.' },
  header: {
    title: 'Postguy',
    logo: { light: '/logo/postguy.svg', dark: '/logo/postguy.svg', alt: 'Postguy', class: 'h-6 w-auto dark:invert' },
  },
  github: { url: 'https://github.com/zuramai/postguy', branch: 'main', rootDir: 'docs' },
  ui: { colors: { primary: 'blue', neutral: 'zinc' } },
})
