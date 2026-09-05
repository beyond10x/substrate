import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    {type: 'doc', id: 'index', label: 'What is Substrate?'},
    {type: 'doc', id: 'getting-started', label: 'Getting started'},
    {type: 'doc', id: 'use-cases', label: 'What you can build'},
    {
      type: 'category',
      label: 'Understand',
      collapsed: false,
      items: [
        {type: 'doc', id: 'concepts/boundary', label: 'System boundary'},
        {type: 'doc', id: 'concepts/model', label: 'System model and derivation'},
        {type: 'doc', id: 'concepts/confinement', label: 'Confinement and refusal'},
        {type: 'doc', id: 'concepts/operations', label: 'Operations and observations'},
      ],
    },
    {
      type: 'category',
      label: 'Operate',
      collapsed: false,
      items: [
        {type: 'doc', id: 'guides/rust-sdk', label: 'Rust SDK'},
        {type: 'doc', id: 'guides/mcp-adapter', label: 'Test through MCP'},
        {type: 'doc', id: 'guides/run-a-command', label: 'Run a bounded command'},
        {type: 'doc', id: 'guides/storage-and-metrics', label: 'Storage and metrics'},
        {type: 'doc', id: 'guides/deployment', label: 'Deployment postures'},
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: [
        {type: 'doc', id: 'reference/contract', label: 'Contract surface'},
      ],
    },
    {type: 'doc', id: 'security', label: 'Security'},
    {type: 'doc', id: 'status', label: 'Status and limitations'},
  ],
};

export default sidebars;
