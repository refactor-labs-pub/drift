// Minimal tab bar primitive. Stateless: the parent owns `value` and
// receives `onChange`. We deliberately don't render the tab *panels*
// here — keeping the bar separate from the panel container lets the
// parent style/wrap each panel however it wants (e.g. some panels need
// scroll containers, others don't).
//
// Why not a routed tab system: each ReportView instance is scoped to
// one event log, and the user expects "switching tabs" to be cheap and
// not change the URL. The router-driven model the static-profile viewer
// uses would conflict with the LiveScan rail's own URL semantics.

import type { ReactNode } from "react";

export interface TabItem<K extends string> {
  key: K;
  label: ReactNode;
  /** Optional trailing chip — e.g. a result count or "live" badge. */
  badge?: ReactNode;
}

export interface TabsProps<K extends string> {
  items: readonly TabItem<K>[];
  value: K;
  onChange: (key: K) => void;
}

export default function Tabs<K extends string>(props: TabsProps<K>): JSX.Element {
  const { items, value, onChange } = props;
  return (
    <div className="report-tabs" role="tablist">
      {items.map((it) => {
        const active = it.key === value;
        return (
          <button
            key={it.key}
            type="button"
            role="tab"
            aria-selected={active}
            className={active ? "report-tab report-tab--active" : "report-tab"}
            onClick={() => onChange(it.key)}
          >
            <span className="report-tab-label">{it.label}</span>
            {it.badge != null && <span className="report-tab-badge">{it.badge}</span>}
          </button>
        );
      })}
    </div>
  );
}
