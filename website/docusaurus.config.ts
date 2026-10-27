import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {withProductSite} from '@beyond10x/docs-system/product-site';

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

export default withProductSite(config, {mark: 'Sb'});
