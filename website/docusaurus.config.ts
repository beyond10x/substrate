import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'Substrate',
  tagline: 'Confined execution with observed outcomes',
  favicon: 'img/favicon.svg',

  future: {
    v4: true,
  },

  url: 'https://beyond10x.github.io',
  baseUrl: '/substrate/',
  organizationName: 'beyond10x',
  projectName: 'substrate',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',
  markdown: {
    format: 'detect',
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          path: './docs',
          routeBasePath: 'docs',
          sidebarPath: './sidebars.ts',
          showLastUpdateTime: true,
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    image: 'img/social-card.svg',
    metadata: [
      {
        name: 'keywords',
        content:
          'execution data plane, process sandbox, confined workspace, durable operations, capability facts, observed state, Rust, Linux',
      },
    ],
    colorMode: {
      defaultMode: 'dark',
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'Substrate',
      hideOnScroll: true,
      logo: {
        alt: 'Substrate mark',
        src: 'img/mark.svg',
      },
      items: [
        {to: '/docs/getting-started', label: 'Get started', position: 'left'},
        {to: '/docs/concepts/boundary', label: 'Boundary', position: 'left'},
        {to: '/docs/concepts/confinement', label: 'Confinement', position: 'left'},
        {to: '/docs/reference/contract', label: 'Contract', position: 'left'},
        {to: '/docs/status', label: 'Status', position: 'left'},
        {
          href: 'https://github.com/beyond10x/substrate',
          label: 'Source',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Understand',
          items: [
            {label: 'What is Substrate?', to: '/docs/'},
            {label: 'System boundary', to: '/docs/concepts/boundary'},
            {label: 'Confinement and refusal', to: '/docs/concepts/confinement'},
            {label: 'Operations and observations', to: '/docs/concepts/operations'},
          ],
        },
        {
          title: 'Use',
          items: [
            {label: 'Run the daemon', to: '/docs/getting-started'},
            {label: 'Deployment postures', to: '/docs/guides/deployment'},
            {label: 'Contract reference', to: '/docs/reference/contract'},
            {label: 'Status and limitations', to: '/docs/status'},
          ],
        },
        {
          title: 'Project',
          items: [
            {label: 'Source', href: 'https://github.com/beyond10x/substrate'},
            {label: 'Apache-2.0 licence', href: 'https://github.com/beyond10x/substrate/blob/main/LICENSE'},
            {label: 'Security', to: '/docs/security'},
            {
              label: 'Report privately',
              href: 'https://github.com/beyond10x/substrate/security/advisories/new',
            },
          ],
        },
        {
          title: 'Explore beyond10x',
          items: [
            {label: 'Start here', href: 'https://beyond10x.github.io/getting-started/'},
            {label: 'Harness', href: 'https://beyond10x.github.io/harness/'},
            {label: 'Engineering Principles', href: 'https://beyond10x.github.io/agentic-principles/'},
            {
              label: 'Engineering Protocols',
              href: 'https://beyond10x.github.io/engineering-protocols/',
            },
          ],
        },
      ],
      copyright:
        '© ' +
        new Date().getFullYear() +
        ' beyond10x · If the machine cannot prove it, Substrate does not claim it.',
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['rust', 'yaml', 'json', 'bash'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
