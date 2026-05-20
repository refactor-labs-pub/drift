interface Props {
  onClick: () => void;
  disabled?: boolean;
  /// Visible label. Defaults to the original "Run magic" brand wording for
  /// any caller that doesn't pass one — keeps existing screenshots / muscle
  /// memory intact for users who haven't seen the new explicit wording.
  /// Default reverts to the original "Make Magic" brand wording — keeps
  /// the primary CTA on Home unmistakable when the caller doesn't pass an
  /// explicit override. Re-run paths on other pages still pass their own
  /// labels (e.g. "Scan entirely new" on the report page).
  label?: string;
  /// Optional one-line caption rendered under the button. Use this to spell
  /// out cache semantics ("Full fresh discovery — discards all caches") so
  /// the user knows what the click is committing to without hover.
  subText?: string;
  /// Optional tooltip — appears on hover, complements `subText`.
  title?: string;
}

export default function RunButton({
  onClick,
  disabled,
  label = "Make Static Magic",
  subText,
  title,
}: Props) {
  return (
    <div className="btn-row">
      <button
        type="button"
        className="run-btn"
        onClick={onClick}
        disabled={disabled}
        title={title}
      >
        <span className="spark">✨</span>
        {label}
      </button>
      {subText && <div className="run-btn-subtext">{subText}</div>}
    </div>
  );
}
