import { CATEGORY_COLORS } from './types';
import { TIPS } from './tooltips';
import { Help } from './Help';
import type { Category, Summary } from './types';

interface Props {
  summary: Summary | null;
  activeCategory?: string | null;
  onToggleCategory?: (cat: string) => void;
}

export function SummaryBar({ summary, activeCategory, onToggleCategory }: Props) {
  if (!summary) return null;
  const cats = Object.entries(summary.categories)
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
  return (
    <div style={barStyle}>
      <Stat label="languages" value={summary.languages.join(', ') || '—'} tip={TIPS.languages} />
      <Stat label="files" value={String(summary.files)} tip={TIPS.files} />
      <Stat label="symbols" value={String(summary.symbols)} tip={TIPS.symbols} />
      <Stat label="edges" value={String(summary.edges)} tip={TIPS.edges} />
      <div style={catsStyle}>
        {cats.length === 0 ? (
          <span style={{ color: '#6e717a', fontStyle: 'italic' }}>no resource calls detected</span>
        ) : (
          cats.map(([cat, n]) => {
            const isActive = activeCategory === cat;
            const isOther = activeCategory && activeCategory !== cat;
            return (
              <button
                key={cat}
                onClick={() => onToggleCategory?.(cat)}
                style={{
                  ...catChipStyle(CATEGORY_COLORS[cat as Category]),
                  cursor: onToggleCategory ? 'pointer' : 'default',
                  opacity: isOther ? 0.35 : 1,
                  outline: isActive ? '2px solid #ffd569' : 'none',
                  outlineOffset: isActive ? 1 : 0,
                  border: 'none',
                  font: 'inherit',
                }}
                title={`${TIPS[`category_${cat}`] ?? ''}\n\n${onToggleCategory ? (isActive ? `Click to clear the ${cat} filter.` : `Click to filter the flame graph to symbols that reach ${cat}.`) : ''}`.trim()}
              >
                <span style={{ fontWeight: 700, textTransform: 'uppercase', fontSize: 10, letterSpacing: 0.3 }}>{cat}</span>
                <span style={{ marginLeft: 6 }}>{n}</span>
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}

function Stat({ label, value, tip }: { label: string; value: string; tip?: string }) {
  // Single tooltip source: the inner <Help> wrapping the label.
  return (
    <div style={{ display: 'flex', gap: 6, alignItems: 'baseline' }}>
      <span style={{ fontSize: 10, color: '#7e8189', textTransform: 'uppercase', letterSpacing: 0.4, display: 'inline-flex', alignItems: 'center' }}>
        {tip ? <Help text={tip}>{label}</Help> : label}
      </span>
      <span style={{ fontSize: 12, color: '#d7d9dc', fontWeight: 500 }}>{value}</span>
    </div>
  );
}

const barStyle: React.CSSProperties = {
  display: 'flex',
  gap: 18,
  alignItems: 'center',
  padding: '8px 16px',
  background: '#26282c',
  borderBottom: '1px solid #3f4147',
  flexWrap: 'wrap',
};
const catsStyle: React.CSSProperties = {
  display: 'flex',
  gap: 6,
  marginLeft: 'auto',
  flexWrap: 'wrap',
};
const catChipStyle = (color: string): React.CSSProperties => ({
  display: 'inline-flex',
  alignItems: 'center',
  padding: '2px 8px',
  borderRadius: 3,
  background: color,
  color: '#0a0a14',
  fontSize: 11,
});
