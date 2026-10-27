import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {withProductSite} from '@beyond10x/docs-system/product-site';
import rehypeTableLabels from './src/rehype/table-labels';

const siteDir = path.dirname(fileURLToPath(import.meta.url));

const config: Config = {
  title: 'Substrate',
  tagline: 'Confined execution, durable operations, observed outcomes.',
  favicon: 'img/favicon.svg',

  future: {
    v4: true,
  },

  url: 'https://beyond10x.github.io',
  baseUrl: '/substrate/',

  organizationName: 'beyond10x',
  projectName: 'substrate',
  deploymentBranch: 'gh-pages',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',

  markdown: {
    format: 'detect',
    mermaid: true,
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  themes: ['@docusaurus/theme-mermaid'],

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          routeBasePath: 'docs',
          editUrl: 'https://github.com/beyond10x/substrate/tree/main/website/',
          rehypePlugins: [rehypeTableLabels],
        },
        blog: false,
        pages: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    navbar: {
      title: 'Substrate',
      logo: {src: 'img/mark.svg', alt: 'Substrate', href: 'pathname:///substrate/'},
      items: [
        {to: '/docs/getting-started', label: 'Get started', position: 'left'},
        {to: '/docs/concepts/adapters', label: 'Adapters', position: 'left'},
        {to: '/docs/reference/contracts', label: 'Contracts', position: 'left'},
        {to: '/docs/releases', label: 'Releases', position: 'left'},
        {href: 'https://github.com/beyond10x/substrate', label: 'Source', position: 'right'},
      ],
    },
    footer: {
      links: [
        {
          title: 'Project',
          items: [
            {label: 'Security', to: '/docs/security'},
            {label: 'Status and limitations', to: '/docs/status'},
            {label: 'Source', href: 'https://github.com/beyond10x/substrate'},
            {label: 'Changelog', href: 'https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md'},
          ],
        },
      ],
      copyright: 'Substrate · Apache-2.0',
    },
  } satisfies Preset.ThemeConfig,
};

const site = withProductSite(config, {mark: 'Sb'});

// Docusaurus writes the whole configuration into the client bundle, and the Docs System names its
// font and share-card directories by absolute path. Docusaurus resolves a static directory against
// the site directory, so the relative form serves the same files and publishes no builder path.
export default {
  ...site,
  staticDirectories: site.staticDirectories?.map((directory) =>
    directory && path.isAbsolute(directory) ? path.relative(siteDir, directory) : directory,
  ),
} satisfies Config;
