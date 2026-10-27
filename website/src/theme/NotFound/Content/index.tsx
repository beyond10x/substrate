import React from 'react';
import Link from '@docusaurus/Link';
import {PageMetadata} from '@docusaurus/theme-common';

// The docs-system not-found content, except that the home link leaves the router: the start page
// is the static website/static/index.html, which no Docusaurus route renders.
export default function NotFoundContent({className}: {className?: string}): React.JSX.Element {
  return (
    <>
      <PageMetadata description="There is no page at this address on Substrate. It may have moved, or the link is out of date.">
        <meta name="robots" content="noindex" />
      </PageMetadata>
      <main className={['container margin-vert--xl b10x-not-found', className].filter(Boolean).join(' ')}>
        <div className="row">
          <div className="col col--6 col--offset-3">
            <p className="b10x-kicker">404</p>
            <h1 className="hero__title">Page not found</h1>
            <p>There is no page at this address. It may have moved, or the link that brought you here is out of date.</p>
            <p className="b10x-not-found__actions">
              <Link className="b10x-button" to="/docs">Go to the documentation</Link>
              <Link className="b10x-text-link" to="pathname:///substrate/">Back to the home page</Link>
            </p>
          </div>
        </div>
      </main>
    </>
  );
}
