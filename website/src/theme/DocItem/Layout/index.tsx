import React from 'react';
import OriginalLayout from '@theme-original/DocItem/Layout';
import type {Props} from '@theme/DocItem/Layout';

// Every docs page published #main as its content anchor before the move to this layer, and the
// route inventory keeps every published anchor.
export default function Layout(props: Props): React.JSX.Element {
  return (
    <div id="main">
      <OriginalLayout {...props} />
    </div>
  );
}
