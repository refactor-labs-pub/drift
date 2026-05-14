import { useMemo } from "react";
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
  type Category,
  type FindingKind,
  type FindingTopRef,
  type Report,
  type Severity,
} from "./types";

/**
 * Drift Lab port of the static-profiler viewer's ScanReport card grid.
 *
 * Same panel set as the viewer (health gauge / findings breakdown / category
 * reach / language breakdown / hot zones / entry points), restyled for the
 * desktop's light Drift Lab palette: warm off-white surfaces, the
 * orange/amber accent gradient from `globals.css`, and a soft severity ramp
 * that reads on `--bg-card`.
 *
 * One file (vs. the viewer's split-out helper components) because there's
 * nothing to share across pages here — the desktop renders one summary on
 * one route. If a future page needs HealthCard standalone we can lift it.
 */
interface Props {
  report: Report;
  onPickEntry?: (entryId: string) => void;
}

export default function ScanSummary({ report, onPickEntry }: Props) {
  const summary = report.summary;
  const findingsByKind = summary.findings_by_kind ?? {};
  const findingsTop = summary.findings_top ?? [];
  const totalFindings = Object.values(findingsByKind).reduce((a, b) => a + b, 0);

  const sevCounts = useMemo(() => {
    const counts: Record<Severity, number> = { high: 0, medium: 0, low: 0 };
    for (const t of findingsTop) counts[t.severity]++;
    return counts;
  }, [findingsTop]);

  // Weighted-sum 10 − (high·0.5 + medium·0.2 + low·0.05) — same formula as
  // the viewer's HealthCard. Keeping the math identical means a saved scan
  // gets the same score in both surfaces.
  const healthScore = useMemo(() => {
    let s = 10;
    s -= sevCounts.high * 0.5;
    s -= sevCounts.medium * 0.2;
    s -= sevCounts.low * 0.05;
    return Math.max(0, s);
  }, [sevCounts]);

  const cats = Object.entries(summary.categories ?? {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]) as [Category, number][];

  const langBreakdown = summary.language_breakdown ?? [];

  return (
    <div className="scan-summary">
      <Header report={report} />
      <div className="scan-summary-grid">
        <HealthCard
          score={healthScore}
          sevCounts={sevCounts}
          totalFindings={totalFindings}
        />
        <FindingsBreakdownCard byKind={findingsByKind} total={totalFindings} />
        <CategoriesCard cats={cats} />
        <LanguagesCard languages={langBreakdown} />
        <HotZonesCard
          zones={findingsTop.filter((t) => t.severity === "high").slice(0, 6)}
        />
        <EntryPointsCard
          entries={report.entries.slice(0, 8)}
          onPickEntry={onPickEntry}
        />
      </div>
    </div>
  );
}

function Header({ report }: { report: Report }) {
  const root = report.generator?.source_root ?? "";
  const base = root ? root.replace(/[/\\]+$/, "").split(/[/\\]/).pop() : null;
  return (
    <div className="scan-summary-head">
      <div className="scan-summary-head-title">
        scan report{base ? ` — .../${base}` : ""}
      </div>
      <div className="scan-summary-head-sub">
        {report.generator?.tool ?? "drift-static-profiler"} {report.generator?.version ?? ""}
        {" · "}
        {report.summary.profiled_language ?? "—"}
        {" · "}
        {report.summary.files} files · {report.summary.symbols} symbols ·{" "}
        {report.summary.edges} edges
      </div>
    </div>
  );
}

function HealthCard({
  score,
  sevCounts,
  totalFindings,
}: {
  score: number;
  sevCounts: Record<Severity, number>;
  totalFindings: number;
}) {
  const pct = score / 10;
  return (
    <Panel title="health score">
      <div className="scan-gauge-row">
        <div className="scan-gauge-outer">
          <div
            className="scan-gauge-fill"
            style={{ width: `${pct * 100}%` }}
          />
        </div>
        <div className="scan-gauge-value">
          {score.toFixed(1)}
          <span className="scan-gauge-denom"> / 10</span>
        </div>
      </div>
      <div className="scan-sev-row">
        <SevPill sev="high" count={sevCounts.high} />
        <SevPill sev="medium" count={sevCounts.medium} />
        <SevPill sev="low" count={sevCounts.low} />
        <span className="scan-sev-total">{totalFindings} total findings</span>
      </div>
    </Panel>
  );
}

function SevPill({ sev, count }: { sev: Severity; count: number }) {
  return (
    <span className="scan-sev-pill">
      <span
        className="scan-mini-badge"
        style={{ background: SEVERITY_COLORS[sev] }}
      >
        {sev}
      </span>
      <strong>{count}</strong>
    </span>
  );
}

function FindingsBreakdownCard({
  byKind,
  total,
}: {
  byKind: Record<string, number>;
  total: number;
}) {
  const rows = Object.entries(byKind)
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
  const max = rows.reduce((m, [, v]) => Math.max(m, v), 1);
  return (
    <Panel title={`findings by kind · ${total} total`}>
      {rows.length === 0 ? (
        <Empty msg="no findings yet" />
      ) : (
        <ul className="scan-list">
          {rows.map(([kind, n]) => (
            <li key={kind} className="scan-row">
              <span className="scan-kind-badge">
                {FINDING_KIND_LABEL[kind as FindingKind] ?? kind}
              </span>
              <span className="scan-bar-cell">
                <span className="scan-bar-outer">
                  <span
                    className="scan-bar-fill"
                    style={{ width: `${(n / max) * 100}%` }}
                  />
                </span>
              </span>
              <span className="scan-count">{n}</span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function CategoriesCard({ cats }: { cats: [Category, number][] }) {
  const max = cats.reduce((m, [, v]) => Math.max(m, v), 1);
  return (
    <Panel title="category reach">
      {cats.length === 0 ? (
        <Empty msg="no resource calls detected" />
      ) : (
        <ul className="scan-list">
          {cats.map(([cat, n]) => (
            <li key={cat} className="scan-row">
              <span
                className="scan-mini-badge"
                style={{ background: CATEGORY_COLORS[cat] }}
              >
                {cat}
              </span>
              <span className="scan-bar-cell">
                <span className="scan-bar-outer">
                  <span
                    className="scan-bar-fill"
                    style={{
                      width: `${(n / max) * 100}%`,
                      background: CATEGORY_COLORS[cat],
                    }}
                  />
                </span>
              </span>
              <span className="scan-count">{n}</span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function LanguagesCard({
  languages,
}: {
  languages: { language: string; percent: number }[];
}) {
  if (languages.length === 0) {
    return (
      <Panel title="language breakdown">
        <Empty msg="—" />
      </Panel>
    );
  }
  return (
    <Panel title="language breakdown">
      <ul className="scan-list">
        {languages.slice(0, 8).map((l) => (
          <li key={l.language} className="scan-row">
            <span className="scan-lang-name">{l.language}</span>
            <span className="scan-bar-cell">
              <span className="scan-bar-outer">
                <span
                  className="scan-bar-fill"
                  style={{ width: `${l.percent}%` }}
                />
              </span>
            </span>
            <span className="scan-count">{l.percent.toFixed(1)}%</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function HotZonesCard({ zones }: { zones: FindingTopRef[] }) {
  if (zones.length === 0) {
    return (
      <Panel title="top hot zones">
        <Empty msg="—" />
      </Panel>
    );
  }
  return (
    <Panel title="top hot zones">
      <ul className="scan-list">
        {zones.map((z, i) => (
          <li key={i} className="scan-row">
            <span
              className="scan-mini-badge"
              style={{ background: SEVERITY_COLORS[z.severity] }}
            >
              {z.severity}
            </span>
            <code className="scan-code">{lastSegment(z.node_id)}</code>
            <span className="scan-loc">
              {fileLineFromId(z.node_id, z.line)}
            </span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function EntryPointsCard({
  entries,
  onPickEntry,
}: {
  entries: Report["entries"];
  onPickEntry?: (entryId: string) => void;
}) {
  if (entries.length === 0) {
    return (
      <Panel title="entry points">
        <Empty msg="—" />
      </Panel>
    );
  }
  return (
    <Panel title={`entry points · ${entries.length}`}>
      <ul className="scan-list">
        {entries.map((e) => (
          <li
            key={e.id}
            className={`scan-row${onPickEntry ? " is-clickable" : ""}`}
            onClick={() => onPickEntry?.(e.id)}
            title={onPickEntry ? `Select ${e.name} as the focused entry` : undefined}
          >
            <code className="scan-code">
              {e.parent_class && (
                <span className="scan-parent">{e.parent_class}.</span>
              )}
              {e.name}
            </code>
            <span className="scan-reach">reach {e.subtree_size}</span>
            <span className="scan-loc">
              {e.file}:{e.line}
            </span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function Panel({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="scan-panel">
      <div className="scan-panel-head">{title}</div>
      <div className="scan-panel-body">{children}</div>
    </div>
  );
}

function Empty({ msg }: { msg: string }) {
  return <div className="scan-empty">{msg}</div>;
}

function lastSegment(id: string): string {
  const parts = id.split("::");
  if (parts.length >= 3) {
    const cls = parts[parts.length - 2];
    const name = parts[parts.length - 1];
    return cls ? `${cls}.${name}` : name;
  }
  return id;
}

function fileLineFromId(id: string, line: number): string {
  const parts = id.split("::");
  return `${parts[0]}:${line}`;
}
