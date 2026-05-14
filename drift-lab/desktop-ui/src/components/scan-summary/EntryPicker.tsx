import type { ScanPickerRoot } from "../../lib/tauri";

/**
 * Inline picker for the top-N entry roots the static analyzer discovered.
 *
 * Rendered on Home as soon as `scan://entries-ready` lands. The flow is
 * blocked until the user clicks a row, at which point we forward the
 * selection through `select_entry_and_scan(scanId, index)` and the parked
 * analysis task resumes building the focused call tree.
 *
 * Standalone so both the live running screen and (potentially) a saved-scan
 * "re-pick" UI can render it without duplicating the markup.
 */
interface Props {
  roots: ScanPickerRoot[];
  onPick: (root: ScanPickerRoot) => void;
  /// Optional header override — Home uses the default, but a future
  /// "switch focus" UI could pass a different label.
  heading?: string;
}

export default function EntryPicker({ roots, onPick, heading }: Props) {
  if (roots.length === 0) {
    return (
      <div className="scan-empty" style={{ marginTop: 16 }}>
        No entry roots discovered for this project.
      </div>
    );
  }
  return (
    <>
      <div className="scan-picker-head">
        {heading ?? `Top ${roots.length} entry roots — pick one to analyze`}
      </div>
      <div className="scan-picker-list">
        {roots.map((r) => (
          <button
            type="button"
            key={r.index}
            className="scan-picker-row"
            onClick={() => onPick(r)}
          >
            <span className="scan-picker-rank">{r.index + 1}.</span>
            <span>
              <div className="scan-picker-name">{r.name}</div>
              <div className="scan-picker-meta">
                {r.file}:{r.line}
                {r.callers.length > 0 && (
                  <>
                    {" · called by "}
                    {r.callers
                      .slice(0, 2)
                      .map((c) => c.name)
                      .join(", ")}
                    {r.callers.length > 2 ? `, +${r.callers.length - 2}` : ""}
                  </>
                )}
              </div>
            </span>
            <span className="scan-picker-reach">reach {r.reach}</span>
          </button>
        ))}
      </div>
    </>
  );
}
