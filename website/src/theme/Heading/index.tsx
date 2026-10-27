import React from 'react';
import OriginalHeading from '@theme-original/Heading';
import type {Props} from '@theme/Heading';
import useBrokenLinks from '@docusaurus/useBrokenLinks';

// The theme drops the ID of an H1. Each page's H1 ID is a published anchor
// (.well-known/b10x-routes.json) that links and redirects into this site already use, so an H1
// keeps the ID its Markdown heading was given.
export default function Heading(props: Props): React.JSX.Element {
  const brokenLinks = useBrokenLinks();
  const {as: As, id, ...rest} = props;
  if (As !== 'h1' || !id) {
    return <OriginalHeading {...props} />;
  }
  brokenLinks.collectAnchor(id);
  return <As {...rest} id={id} />;
}
