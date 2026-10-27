// UXF-004: a table's scrolling frame is named by the heading above it. The page is rendered on the
// server, where no component can look back at the document, so this pass writes that heading's
// text onto each table as `data-table-label` before the page is compiled; the frame in
// src/theme/MDXComponents reads it and drops it from the table.
type Node = {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
};

const HEADING = /^h[1-6]$/;

function text(node: Node): string {
  if (node.type === 'text') return node.value ?? '';
  return (node.children ?? []).map(text).join('');
}

export default function rehypeTableLabels() {
  return (tree: Node): void => {
    let heading = '';
    const visit = (node: Node): void => {
      if (node.type === 'element' && HEADING.test(node.tagName ?? '')) {
        heading = text(node).trim();
        return;
      }
      if (node.type === 'element' && node.tagName === 'table' && heading) {
        node.properties = {...node.properties, dataTableLabel: heading};
      }
      (node.children ?? []).forEach(visit);
    };
    visit(tree);
  };
}
