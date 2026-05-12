import { CATEGORY_COLORS } from './types';
import { TIPS } from './tooltips';
import { Help } from './Help';
import type { Category, HotPath } from './types';

interface Props {
  paths: HotPath[];
  onJump?: (frame: string) => void;
}

export function HotPaths({ paths, onJump }: Props) {
  if (paths.length === 0) {
    return (
      <div style={{ padding: 12, color: '#6e717a', fontStyle: 'italic', fontSize: 12 }}>
        no hot paths — analyzer didn't reach any classified resource calls.
      </div>
    );
  }
  return (
    <div style={containerStyle}>
      <div style={hintStyle}>
        {TIPS.hot_path} <Help text={TIPS.hot_path} />
      </div>
      {paths.map((p, i) => (
        <div key={i} style={pathStyle}>
          <span
            style={badgeStyle(CATEGORY_COLORS[p.terminal_category as Category])}
            title={`${TIPS.terminal_category}\n\n${TIPS[`category_${p.terminal_category}`] ?? ''}`}
          >
            {p.terminal_category}
          </span>
          <span style={depthStyle} title={TIPS.hot_path_depth}>depth {p.depth}</span>
          <span style={framesStyle}>
            {p.frames.map((f, j) => (
              <span key={j}>
                <code
                  style={onJump ? frameClickableStyle : frameStyle}
                  onClick={() => onJump?.(f)}
                  title={onJump ? `jump to ${f}` : undefined}
                >
                  {f}
                </code>
                {j < p.frames.length - 1 && <span style={arrowStyle}> → </span>}
              </span>
            ))}
          </span>
        </div>
      ))}
    </div>
  );
}

const containerStyle: React.CSSProperties = {
  padding: '8px 12px',
  overflowY: 'auto',
  fontSize: 12,
};
const pathStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '4px 0',
  borderBottom: '1px solid #2f3136',
};
const badgeStyle = (color: string): React.CSSProperties => ({
  display: 'inline-block',
  padding: '2px 8px',
  borderRadius: 3,
  background: color,
  color: '#0a0a14',
  fontSize: 10,
  fontWeight: 700,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
  minWidth: 50,
  textAlign: 'center',
});
const depthStyle: React.CSSProperties = { color: '#7e8189', fontSize: 11, minWidth: 60 };
const framesStyle: React.CSSProperties = { color: '#d7d9dc', fontFamily: 'ui-monospace, monospace' };
const frameStyle: React.CSSProperties = { background: '#2f3136', padding: '1px 5px', borderRadius: 2, color: '#d7d9dc' };
const frameClickableStyle: React.CSSProperties = {
  ...frameStyle,
  cursor: 'pointer',
  textDecoration: 'underline',
  textDecorationColor: '#5b8def',
  textUnderlineOffset: 2,
};
const hintStyle: React.CSSProperties = {
  color: '#6e717a', fontSize: 10, fontStyle: 'italic', marginBottom: 6,
};
const arrowStyle: React.CSSProperties = { color: '#6e717a', margin: '0 4px' };
