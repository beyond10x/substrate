import React, {useEffect, useId, useRef, useState, type ComponentProps} from 'react';
import MDXComponents from '@theme-original/MDXComponents';

type TableProps = ComponentProps<'table'> & {'data-table-label'?: string};

// UXF-004: every table scrolls inside a region a keyboard can reach, named by the heading above it
// (src/rehype/table-labels.ts writes that heading onto the table), in the server-rendered page and
// not only after hydration. The frame keeps the Docs System's class, so it looks and scrolls as
// the Docs System's own table does; the guidance line shows only while the table overflows.
function Table({'data-table-label': heading, children, ...props}: TableProps): React.JSX.Element {
  const viewport = useRef<HTMLDivElement>(null);
  const instructionsId = `b10x-table-${useId().replaceAll(':', '')}-instructions`;
  const [overflow, setOverflow] = useState(false);
  useEffect(() => {
    const element = viewport.current;
    if (!element) return undefined;
    const measure = () => setOverflow(element.scrollWidth > element.clientWidth + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    const table = element.querySelector('table');
    if (table) observer.observe(table);
    return () => observer.disconnect();
  }, []);
  return (
    <>
      <div
        ref={viewport}
        className="b10x-table-wrap"
        role="region"
        tabIndex={0}
        aria-label={heading ? `${heading} table` : 'Table'}
        aria-describedby={overflow ? instructionsId : undefined}>
        <table {...props}>{children}</table>
      </div>
      <p className="b10x-table-guidance" id={instructionsId} hidden={!overflow} data-pagefind-ignore>
        More columns: swipe horizontally, or focus the table and use the arrow keys.
      </p>
    </>
  );
}

export default {...MDXComponents, table: Table};
