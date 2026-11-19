import { defineConfig } from 'vitepress'
import conduitGrammar from '../../editors/textmate/conduit.tmLanguage.json'

const libraryItems = [
  { text: 'Overview', link: '/library/' },
  { text: 'cdt.json', link: '/library/json' },
  { text: 'cdt.time', link: '/library/time' },
  { text: 'cdt.validation', link: '/library/validation' },
  { text: 'cdt.env', link: '/library/env' },
  { text: 'cdt.cli', link: '/library/cli' },
  { text: 'cdt.fs: reading and buffered writing', link: '/library/fs' },
  { text: 'cdt.path', link: '/library/path' },
  { text: 'cdt.process', link: '/library/process' },
  { text: 'cdt.crypto', link: '/library/crypto' },
  { text: 'cdt.url', link: '/library/url' },
  { text: 'cdt.csv', link: '/library/csv' },
  { text: 'cdt.math', link: '/library/math' },
  { text: 'cdt.log', link: '/library/log' },
  { text: 'cdt.http', link: '/library/http' },
  { text: 'cdt.db', link: '/library/db' },
  { text: 'cdt.cron', link: '/library/cron' },
  { text: 'cdt.jwt', link: '/library/jwt' },
  { text: 'cdt.lock', link: '/library/lock' },
  { text: 'cdt.cache', link: '/library/cache' },
  { text: 'cdt.redis', link: '/library/redis' },
]

export default defineConfig({
  lang: 'en-US',
  title: 'The Conduit Programming Language',
  description: 'Official documentation for the Conduit programming language',
  base: '/conduit/',
  cleanUrls: false,
  srcDir: 'src',
  srcExclude: ['**/SUMMARY.md'],
  markdown: {
    languages: [
      {
        ...conduitGrammar,
        name: 'conduit',
        displayName: 'Conduit',
        aliases: ['cdt'],
      },
    ],
  },
  themeConfig: {
    nav: [
      { text: 'Introduction', link: '/introduction' },
      { text: 'Getting Started', link: '/getting-started/installation' },
      { text: 'Standard Library', link: '/library/' },
    ],
    sidebar: [
      { text: 'Introduction', link: '/introduction' },
      {
        text: 'Getting Started',
        items: [
          { text: 'Installation', link: '/getting-started/installation' },
          { text: 'Your First Program', link: '/getting-started/first-program' },
          { text: 'Command Line Interface', link: '/getting-started/cli' },
          { text: 'Editor Setup', link: '/getting-started/editors' },
        ],
      },
      {
        text: 'Language Reference',
        items: [
          { text: 'Variables & Mutability', link: '/language/variables' },
          { text: 'Types & Records', link: '/language/types' },
          { text: 'Functions', link: '/language/functions' },
          { text: 'Control Flow', link: '/language/control-flow' },
          { text: 'Collections', link: '/language/collections' },
          { text: 'Errors & Unions', link: '/language/errors' },
          { text: 'Tuples', link: '/language/tuples' },
          { text: 'Modules & Privacy', link: '/language/modules' },
        ],
      },
      { text: 'Standard Library', items: libraryItems },
      {
        text: 'Practical Guides',
        items: [
          { text: 'Working with Errors', link: '/guides/working-with-errors' },
          { text: 'Structuring Modules', link: '/guides/creating-modules' },
        ],
      },
      {
        text: 'Internals',
        items: [{ text: 'Architecture & Pipeline', link: '/internals/architecture' }],
      },
    ],
    search: { provider: 'local' },
    editLink: {
      pattern: 'https://github.com/resoul/conduit/edit/main/docs/src/:path',
      text: 'Edit this page on GitHub',
    },
    socialLinks: [{ icon: 'github', link: 'https://github.com/resoul/conduit' }],
  },
})
