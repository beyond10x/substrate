import {useState, type ReactNode} from 'react';
import Link from '@docusaurus/Link';
import Layout from '@theme/Layout';
import Heading from '@theme/Heading';

import styles from './index.module.css';

const quickCommand =
  'curl --unix-socket ./run/substrate.sock http://localhost/v1/machine';

const pathways = [
  {
    number: '01',
    label: 'Start locally',
    detail: 'inspect before execution',
    to: '/docs/getting-started',
  },
  {
    number: '02',
    label: 'Find the boundary',
    detail: 'intent stops, facts begin',
    to: '/docs/concepts/boundary',
  },
  {
    number: '03',
    label: 'Read the guarantees',
    detail: 'confinement or refusal',
    to: '/docs/concepts/confinement',
  },
  {
    number: '04',
    label: 'Inspect the wire',
    detail: 'resources and recovery',
    to: '/docs/reference/contract',
  },
];

const operationSteps = [
  ['01', 'Reserve', 'persist the operation before an effect'],
  ['02', 'Verify', 'bind admission to probed capabilities'],
  ['03', 'Dispatch', 'perform one bounded driver action'],
  ['04', 'Observe', 're-read what the machine can prove'],
  ['05', 'Record', 'commit outcome, event and evidence'],
];

const outcomeCards = [
  {
    code: 'exec.sandbox-unavailable',
    title: 'Unavailable is not weaker',
    text: 'If the host cannot enforce the complete execution floor, the process never starts.',
    tone: 'refused',
  },
  {
    code: 'exited { code: 1 }',
    title: 'Exit is an observation',
    text: 'A non-zero child exit can be a successful API operation with a truthful terminal result.',
    tone: 'observed',
  },
  {
    code: 'outcome: unknown',
    title: 'Unknown stays unknown',
    text: 'After an unanswered effect or restart, missing proof is not rewritten as success or failure.',
    tone: 'unknown',
  },
  {
    code: 'stdout: truncated',
    title: 'Bounds remain visible',
    text: 'Output is capped without stopping the drain, and truncation survives in the record.',
    tone: 'bounded',
  },
];

function QuickCommand(): ReactNode {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');

  async function copyCommand() {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(quickCommand);
      } else {
        const copyTarget = document.createElement('textarea');
        copyTarget.value = quickCommand;
        copyTarget.style.position = 'fixed';
        copyTarget.style.opacity = '0';
        document.body.append(copyTarget);
        copyTarget.select();
        const copied = document.execCommand('copy');
        copyTarget.remove();
        if (!copied) throw new Error('Copy command was unavailable');
      }
      setCopyState('copied');
      window.setTimeout(() => setCopyState('idle'), 1800);
    } catch {
      setCopyState('failed');
    }
  }

  const copyLabel =
    copyState === 'copied' ? 'Copied' : copyState === 'failed' ? 'Copy failed' : 'Copy';

  return (
    <div className={styles.quickCommand}>
      <span className={styles.commandPrompt} aria-hidden="true">$</span>
      <code>{quickCommand}</code>
      <button type="button" onClick={copyCommand} aria-label="Copy the machine facts command">
        <span aria-live="polite">{copyLabel}</span>
        <i aria-hidden="true" />
      </button>
    </div>
  );
}

function MachinePanel(): ReactNode {
  const [profile, setProfile] = useState<'confined' | 'limited'>('confined');
  const confined = profile === 'confined';

  return (
    <aside className={styles.machinePanel} aria-label="A verified Substrate capability snapshot">
      <div className={styles.panelBar}>
        <span>machine / local:1000</span>
        <span className={styles.probed}><i /> probed now</span>
      </div>
      <div className={styles.profilePicker}>
        <span>HOST PROFILE</span>
        <div role="tablist" aria-label="Compare host capability profiles">
          <button
            type="button"
            role="tab"
            aria-selected={confined}
            onClick={() => setProfile('confined')}>
            Confined
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={!confined}
            onClick={() => setProfile('limited')}>
            Limited
          </button>
        </div>
      </div>
      <div className={styles.panelIdentity}>
        <span>DRIVER</span>
        <strong>linux-host</strong>
        <small>generation / 7f3a…91</small>
      </div>
      <ul className={styles.factList}>
        <li>
          <span>file.guard</span>
          <strong>openat2 / beneath</strong>
          <b>served</b>
        </li>
        <li className={confined ? '' : styles.factAbsent}>
          <span>exec.confined</span>
          <strong>{confined ? 'bwrap + cgroup v2' : 'delegation absent'}</strong>
          <b>{confined ? 'served' : 'absent'}</b>
        </li>
        <li className={confined ? '' : styles.factAbsent}>
          <span>network.egress</span>
          <strong>{confined ? 'none / verified' : 'not probed'}</strong>
          <b>{confined ? 'served' : 'absent'}</b>
        </li>
        <li>
          <span>operation.ledger</span>
          <strong>durable before dispatch</strong>
          <b>served</b>
        </li>
      </ul>
      <div className={confined ? styles.panelOutcome : styles.panelRefusal}>
        <span>{confined ? 'ADMISSION' : 'NAMED REFUSAL'}</span>
        <p aria-live="polite">
          {confined
            ? 'The requested guarantees are present. Dispatch may proceed.'
            : 'exec.sandbox-unavailable · no process was started.'}
        </p>
      </div>
      <div className={styles.panelFoot}>
        <span>one trust domain</span>
        <span>observed_at / now</span>
      </div>
    </aside>
  );
}

export default function Home(): ReactNode {
  return (
    <Layout
      title="Confined execution with observed outcomes"
      description="Substrate is the b10x execution data plane: confined workspaces, bounded processes, durable operations, verified capability facts, and observed state.">
      <main>
        <header className={styles.hero}>
          <div className={styles.heroGlow} />
          <div className={['container', styles.heroGrid].join(' ')}>
            <div className={styles.heroCopy}>
              <p className={styles.eyebrow}><span /> EXECUTION DATA PLANE / OBSERVED STATE</p>
              <Heading as="h1">Run things. <em>Claim only what happened.</em></Heading>
              <p className={styles.lede}>
                Turn one Linux machine into a governed service for confined workspaces and bounded
                processes. Admit only what the host can enforce—and keep the observation when the
                caller, process, or connection disappears.
              </p>
              <div className={styles.actions}>
                <Link className={styles.primaryAction} to="/docs/getting-started">
                  Inspect your machine <span aria-hidden="true">↗</span>
                </Link>
                <Link className={styles.secondaryAction} to="/docs/concepts/boundary">
                  Understand the boundary
                </Link>
              </div>
              <div className={styles.commandWrap}>
                <span>Ask the running daemon what it can prove</span>
                <QuickCommand />
              </div>
              <div className={styles.metrics} aria-label="Substrate at a glance">
                <div><strong>1</strong><span>machine scope</span></div>
                <div><strong>0</strong><span>silent fallbacks</span></div>
                <div><strong>1</strong><span>driver-neutral contract</span></div>
              </div>
            </div>
            <MachinePanel />
          </div>
        </header>

        <nav className={styles.pathways} aria-label="Explore Substrate documentation">
          <div className="container">
            <span className={styles.pathwaysLabel}>CHOOSE A PATH</span>
            <div className={styles.pathwaysGrid}>
              {pathways.map((path) => (
                <Link to={path.to} key={path.number}>
                  <small>{path.number}</small>
                  <span><strong>{path.label}</strong><em>{path.detail}</em></span>
                  <b aria-hidden="true">↗</b>
                </Link>
              ))}
            </div>
          </div>
        </nav>

        <section className={styles.thesis}>
          <div className="container">
            <p className={styles.sectionLabel}>THE LINE IN THE SYSTEM</p>
            <Heading as="h2">Intent stops here. Machine facts begin.</Heading>
            <div className={styles.thesisGrid}>
              <p>
                A product, agent, or automation decides why an action should happen. Substrate owns
                the execution mechanics: guarded paths, argv-only spawn, sandbox admission, durable
                lifecycle, output bounds, cancellation and observed state.
              </p>
              <p>
                It does not schedule a fleet or decide product policy. The deliberately smaller
                boundary makes one claim testable: every served effect is either enforced and
                observed, or refused by name.
              </p>
            </div>
            <div className={styles.boundaryRail} aria-label="The Substrate system boundary">
              <div><span>01</span><strong>caller</strong><small>intent + policy</small></div>
              <i aria-hidden="true">→</i>
              <div className={styles.boundaryCore}><span>02</span><strong>Substrate</strong><small>admit + persist</small></div>
              <i aria-hidden="true">→</i>
              <div><span>03</span><strong>driver</strong><small>enforce + probe</small></div>
              <i aria-hidden="true">→</i>
              <div><span>04</span><strong>observation</strong><small>facts + events</small></div>
            </div>
          </div>
        </section>

        <section className={styles.operation}>
          <div className="container">
            <div className={styles.sectionHead}>
              <div>
                <p className={styles.sectionLabel}>DURABLE BEFORE EFFECT</p>
                <Heading as="h2">A mutation leaves a recovery path.</Heading>
              </div>
              <Link to="/docs/concepts/operations">Read the operation model →</Link>
            </div>
            <ol className={styles.operationRail}>
              {operationSteps.map(([number, title, text]) => (
                <li key={number}>
                  <span>{number}</span>
                  <Heading as="h3">{title}</Heading>
                  <p>{text}</p>
                </li>
              ))}
            </ol>
          </div>
        </section>

        <section className={styles.outcomes}>
          <div className="container">
            <div className={styles.sectionHead}>
              <div>
                <p className={styles.sectionLabel}>FAILURE IS DATA</p>
                <Heading as="h2">The record does not tidy up reality.</Heading>
              </div>
              <Link to="/docs/concepts/confinement">Inspect the guarantees →</Link>
            </div>
            <div className={styles.outcomeGrid}>
              {outcomeCards.map((card) => (
                <article key={card.code} className={styles[card.tone]}>
                  <code>{card.code}</code>
                  <Heading as="h3">{card.title}</Heading>
                  <p>{card.text}</p>
                </article>
              ))}
            </div>
          </div>
        </section>

        <section className={styles.layers}>
          <div className={['container', styles.layersGrid].join(' ')}>
            <div className={styles.layersCopy}>
              <p className={styles.sectionLabel}>SMALL ON PURPOSE</p>
              <Heading as="h2">Execution mechanics, without product policy.</Heading>
              <p>
                Higher layers bring intent, approval, placement and identity. Substrate contributes
                one narrow thing they should not each rebuild: an execution boundary that knows
                which guarantees the selected machine can actually keep.
              </p>
              <div className={styles.inlineActions}>
                <Link to="/docs/concepts/boundary">Explore the boundary ↗</Link>
                <Link to="/docs/guides/deployment">Choose a posture</Link>
              </div>
            </div>
            <div className={styles.layerStack} aria-label="Responsibility layers around Substrate">
              <article>
                <span>INTENT LAYER</span>
                <Heading as="h3">Products · agents · automation</Heading>
                <p>why, who, where, approval</p>
              </article>
              <article className={styles.layerActive}>
                <span>EXECUTION DATA PLANE</span>
                <Heading as="h3">Substrate</Heading>
                <p>resources, bounds, operations, observations</p>
              </article>
              <article>
                <span>ENFORCEMENT LAYER</span>
                <Heading as="h3">Verified machine driver</Heading>
                <p>filesystem, process, isolation, facts</p>
              </article>
            </div>
          </div>
        </section>

        <section className={styles.now}>
          <div className="container">
            <div>
              <p className={styles.sectionLabel}>THE CURRENT LINE</p>
              <Heading as="h2">Linux host slice. Development contract. Explicit gaps.</Heading>
            </div>
            <p>
              Guarded workspaces, capability-gated exec, durable operations, events, leases and a
              leased raw-pipe and probe-gated PTY development slice are served today. Docker,
              Kubernetes, workloads, images, Git sources and stable signed packaging are absent.
            </p>
            <Link to="/docs/status">Read status and limitations <span aria-hidden="true">↗</span></Link>
          </div>
        </section>
      </main>
    </Layout>
  );
}
