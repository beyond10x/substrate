import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const page = (id: string, label: string) => ({type: 'doc' as const, id, label});

const sidebars: SidebarsConfig = {
  docsSidebar: [
    {
      type: 'category',
      label: 'Start',
      collapsed: false,
      items: [page('index', 'Overview'), page('getting-started', 'Get started'), page('use-cases', 'Use cases')],
    },
    {
      type: 'category',
      label: 'Concepts',
      collapsed: false,
      items: [
        page('concepts/boundary', 'System boundary'),
        page('concepts/confinement', 'Confinement'),
        page('concepts/adapters', 'Confinement adapters'),
        page('concepts/operations', 'Operations'),
        page('concepts/model', 'System model'),
      ],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: [
        page('guides/run-a-command', 'Run a command'),
        page('guides/confinement-profiles', 'Confinement profiles'),
        page('guides/storage-and-metrics', 'Storage and metrics'),
        page('guides/deployment', 'Deployment'),
        page('guides/rust-sdk', 'Rust SDK'),
        page('guides/cli', 'Command line'),
        page('guides/mcp-adapter', 'MCP adapter'),
        page('guides/remote-conformance', 'Remote conformance'),
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: [
        page('reference/contracts', 'Contracts overview'),
        page('reference/contract', 'Wire contract'),
        page('releases', 'Release notes'),
      ],
    },
    {
      type: 'category',
      label: 'Project',
      collapsed: false,
      items: [page('status', 'Status'), page('security', 'Security')],
    },
  ],
};

export default sidebars;
