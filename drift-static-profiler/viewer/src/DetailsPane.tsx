import { CATEGORY_COLORS } from './types';
import { TIPS } from './tooltips';
import { Help } from './Help';
import type { CallTreeNode, Category } from './types';

interface Props {
  node: CallTreeNode | null;
  onJumpTo: (id: string) => void;
  onJumpExternal?: (file: string, line: number) => void;
}

export function DetailsPane({ node, onJumpTo, onJumpExternal }: Props) {
  if (!node) {
    return (
      <div style={paneStyle}>
        <div style={headerStyle}>Selected</div>
        <div style={emptyStyle}>Click a frame or row to inspect.</div>
      </div>
    );
  }
  const reached = Object.entries(node.categories_reached || {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]) as [Category, number][];

  return (
    <div style={paneStyle}>
      <div style={headerStyle}>Selected</div>
      <Row k="name" v={node.name} mono />
      {node.parent_class && <Row k="class" v={node.parent_class} mono tip="Enclosing class — methods belong to a class; top-level functions don't." />}
      <Row k="kind" v={node.kind} tip={node.kind === 'Function' ? TIPS.kind_function : node.kind === 'Method' ? TIPS.kind_method : TIPS.kind_class} />
      <Row k="file" v={`${node.file}:${node.line}`} mono />

      <RowBadges
        k="self"
        tip={TIPS.category_self}
        badges={node.category_self ? [{ label: node.category_self, color: CATEGORY_COLORS[node.category_self], tip: TIPS[`category_${node.category_self}`] }] : []}
      />
      <RowBadges
        k="reaches"
        tip={TIPS.categories_reached}
        badges={reached.map(([c, n]) => ({ label: `${c} × ${n}`, color: CATEGORY_COLORS[c], tip: TIPS[`category_${c}`] }))}
      />

      {/* Smells */}
      {(node.n_plus_one_risk || node.blocking_in_async || node.is_recursive) && (
        <div style={smellsRowStyle}>
          <span style={keyStyle}>
            smells <Help text="Detected static code smells. Hover each badge for the antipattern definition and how to fix it." />
          </span>
          <span style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
            {node.n_plus_one_risk && <Badge label="N+1 RISK" color="#e26d6d" tip={TIPS.smell_n_plus_one} />}
            {node.blocking_in_async && <Badge label="BLOCKING IN ASYNC" color="#ff7e7e" tip={TIPS.smell_blocking} />}
            {node.is_recursive && <Badge label="RECURSIVE" color="#d09bd1" tip={TIPS.smell_recursive} />}
          </span>
        </div>
      )}

      <Row k="depth" v={String(node.depth)} tip="How deep this node sits below the entry point. 0 = the entry itself." />
      <Row k="% total" v={`${node.percent_total?.toFixed(1) ?? '0.0'}%`} tip={TIPS.percent_total} />
      <Row k="% parent" v={`${node.percent_parent?.toFixed(1) ?? '0.0'}%`} tip={TIPS.percent_parent} />
      <Row k="callers" v={`${node.callers_count} (${node.call_site_count} sites)`} tip={`${TIPS.callers_count}\n\nIn parens: ${TIPS.call_site_count}`} />
      <Row k="callees" v={String(node.callees_count)} tip={TIPS.callees_count} />
      <Row k="subtree" v={String(node.subtree_size)} tip={TIPS.subtree_size} />
      <Row k="complexity" v={String(node.complexity ?? 0)} tip={TIPS.complexity} />
      <Row k="loc" v={String(node.loc ?? 0)} tip={TIPS.loc} />
      <Row k="nesting" v={String(node.nesting_depth ?? 0)} tip={TIPS.nesting_depth} />
      <Row k="params" v={String(node.parameter_count ?? 0)} tip={TIPS.parameter_count} />
      <Row k="async" v={node.is_async ? 'yes' : 'no'} tip={TIPS.is_async} />
      <Row k="pagerank" v={node.pagerank?.toFixed(4) ?? '0.0000'} tip={TIPS.pagerank} />
      {node.truncated_reason && (
        <Row
          k="truncated"
          v={node.truncated_reason}
          highlight
          tip={node.truncated_reason === 'cycle' ? TIPS.truncated_cycle : TIPS.truncated_maxdepth}
        />
      )}

      {/* Callers section */}
      <SectionHeader tip={TIPS.callers_count}>callers (incoming · {node.callers.length})</SectionHeader>
      {node.callers.length === 0 ? (
        <div style={emptyStyle}>no callers — likely an entry point</div>
      ) : (
        <ul style={listStyle}>
          {node.callers.map((c) => (
            <li key={c.id} style={liButtonStyle} onClick={() => onJumpTo(c.id)} title="jump to this caller">
              <span style={{ color: '#9ca0a8' }}>
                {c.parent_class ? `${c.parent_class}.` : ''}
              </span>
              {c.name}
              <span style={locStyle}>{c.file}:{c.line}</span>
            </li>
          ))}
        </ul>
      )}

      {/* Direct callees */}
      <SectionHeader tip={TIPS.callees_count}>callees (outgoing · {node.children.length})</SectionHeader>
      {node.children.length === 0 && node.external_calls.length === 0 ? (
        <div style={emptyStyle}>leaf — no further calls</div>
      ) : (
        <ul style={listStyle}>
          {node.children.map((c) => (
            <li key={c.id} style={liButtonStyle} onClick={() => onJumpTo(c.id)} title="jump to this callee">
              <span style={{ color: '#9ca0a8' }}>
                {c.parent_class ? `${c.parent_class}.` : ''}
              </span>
              {c.name}
              {c.category_self && (
                <Badge label={c.category_self} color={CATEGORY_COLORS[c.category_self]} small tip={TIPS[`category_${c.category_self}`]} />
              )}
              <span style={locStyle}>{c.file}:{c.line}</span>
            </li>
          ))}
        </ul>
      )}

      {/* External calls */}
      {node.external_calls.length > 0 && (
        <>
          <SectionHeader tip={TIPS.external_calls}>external resource calls ({node.external_calls.length})</SectionHeader>
          <ul style={listStyle}>
            {node.external_calls.map((e, i) => {
              const tierTip = e.tier === 'imported_module' ? TIPS.tier_imported_module
                : e.tier === 'receiver_pattern' ? TIPS.tier_receiver_pattern
                : e.tier === 'method_signature' ? TIPS.tier_method_signature
                : '';
              const tooltip = [
                tierTip || null,
                e.evidence ? `Evidence: ${e.evidence}` : null,
                e.in_loop ? '⚠ inside a loop — N+1 candidate' : null,
                e.in_await ? '✓ awaited (non-blocking)' : null,
              ].filter(Boolean).join('\n\n');
              const clickable = !!onJumpExternal;
              return (
                <li
                  key={`${e.name}-${e.line}-${i}`}
                  style={clickable ? liButtonStyle : liStyle}
                  onClick={() => clickable && onJumpExternal!(node.file, e.line)}
                >
                  <Badge label={e.category} color={CATEGORY_COLORS[e.category]} small tip={TIPS[`category_${e.category}`]} />
                  <span style={{ fontFamily: 'ui-monospace, monospace' }}>
                    {e.receiver ? <span style={{ color: '#7e8189' }}>{e.receiver}.</span> : null}
                    {e.name}()
                  </span>
                  {e.in_loop && <Help text={TIPS.in_loop}><span style={extTagDangerStyle}>loop</span></Help>}
                  {e.in_await && <Help text={TIPS.in_await}><span style={extTagOkStyle}>await</span></Help>}
                  <span style={locStyle}>:{e.line}</span>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
  );
}

function SectionHeader({ children, tip }: { children: React.ReactNode; tip?: string }) {
  return (
    <div style={subHeaderStyle}>
      {tip ? <Help text={tip}>{children}</Help> : children}
    </div>
  );
}

function Row({ k, v, mono, highlight, tip }: { k: string; v: string; mono?: boolean; highlight?: boolean; tip?: string }) {
  // Note: no title= attribute on the row div; the inner <Help> is the single
  // source of tooltip truth.
  return (
    <div style={rowStyle}>
      <span style={keyStyle}>
        {tip ? <Help text={tip}>{k}</Help> : k}
      </span>
      <span style={{ ...valueStyle, fontFamily: mono ? 'ui-monospace, monospace' : 'inherit', color: highlight ? '#ff7e7e' : valueStyle.color }}>
        {v}
      </span>
    </div>
  );
}

function RowBadges({ k, tip, badges }: { k: string; tip?: string; badges: { label: string; color: string; tip?: string }[] }) {
  return (
    <div style={rowStyle}>
      <span style={keyStyle}>
        {tip ? <Help text={tip}>{k}</Help> : k}
      </span>
      <span style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
        {badges.length === 0 ? <span style={{ color: '#6e717a', fontStyle: 'italic' }}>none</span> : badges.map(b => (
          <Badge key={b.label} label={b.label} color={b.color} tip={b.tip} />
        ))}
      </span>
    </div>
  );
}

function Badge({ label, color, small, tip }: { label: string; color: string; small?: boolean; tip?: string }) {
  return (
    <span
      title={tip}
      style={{
        display: 'inline-block',
        padding: small ? '0 5px' : '2px 7px',
        borderRadius: 3,
        background: color,
        color: '#0a0a14',
        fontSize: small ? 10 : 11,
        fontWeight: 700,
        textTransform: 'uppercase',
        letterSpacing: 0.3,
        cursor: tip ? 'help' : 'default',
      }}
    >{label}</span>
  );
}

const paneStyle: React.CSSProperties = {
  background: '#26282c',
  borderLeft: '1px solid #3f4147',
  padding: 14,
  height: '100%',
  overflowY: 'auto',
};
const headerStyle: React.CSSProperties = { fontWeight: 600, fontSize: 12, color: '#9ca0a8', textTransform: 'uppercase', letterSpacing: 0.5, marginBottom: 10 };
const subHeaderStyle: React.CSSProperties = { ...headerStyle, marginTop: 14, marginBottom: 6, fontSize: 10 };
const rowStyle: React.CSSProperties = { display: 'flex', gap: 12, padding: '4px 0', borderBottom: '1px solid #2f3136', fontSize: 12, alignItems: 'center' };
const keyStyle: React.CSSProperties = { width: 70, color: '#7e8189', display: 'flex', alignItems: 'center' };
const valueStyle: React.CSSProperties = { color: '#d7d9dc', wordBreak: 'break-all', flex: 1 };
const emptyStyle: React.CSSProperties = { color: '#6e717a', fontSize: 11, fontStyle: 'italic', padding: '4px 0' };
const listStyle: React.CSSProperties = { listStyle: 'none', padding: 0, margin: 0, fontFamily: 'ui-monospace, monospace', fontSize: 11 };
const liStyle: React.CSSProperties = { padding: '3px 0', color: '#d7d9dc', display: 'flex', alignItems: 'center', gap: 6 };
const liButtonStyle: React.CSSProperties = { ...liStyle, cursor: 'pointer', padding: '3px 4px', borderRadius: 3 };
const locStyle: React.CSSProperties = { marginLeft: 'auto', color: '#6e717a', fontSize: 10 };
const extTagDangerStyle: React.CSSProperties = {
  fontSize: 9, padding: '0 4px', borderRadius: 2,
  background: '#3a2630', color: '#e26d6d', fontWeight: 700,
  textTransform: 'uppercase', letterSpacing: 0.3, cursor: 'help',
};
const extTagOkStyle: React.CSSProperties = {
  fontSize: 9, padding: '0 4px', borderRadius: 2,
  background: '#263a30', color: '#48a999', fontWeight: 700,
  textTransform: 'uppercase', letterSpacing: 0.3, cursor: 'help',
};
const smellsRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 12,
  padding: '6px 0',
  borderBottom: '1px solid #2f3136',
  background: '#3a2630',
  borderRadius: 4,
  paddingLeft: 8,
  marginTop: 6,
};
