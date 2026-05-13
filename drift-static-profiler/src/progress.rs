//! Progress reporting seam for the analysis pipeline.
//!
//! The CLI's `scan` / `analyze-root` commands can run for minutes on a
//! large monorepo. Without progress callbacks, the process appears to
//! hang. We solve this without coupling the library to a TTY writer or
//! a progress-bar crate: `Progress` is a small trait the orchestrator
//! calls at well-defined moments, and library consumers can plug in
//! either [`NullProgress`] (no output) or any custom implementation.
//!
//! The CLI's bar implementation lives next to `main.rs` and is wired
//! in only at the binary layer, so the library crate stays free of
//! terminal-output concerns.
//!
//! Design follows Open/Closed: new pipeline phases can call additional
//! methods on `Progress` (provide a default no-op body), and existing
//! consumers keep working without recompiling.
//!
//! Thread-safety: implementations are called from both the walker
//! callback (single-threaded today) and from inside rayon's parallel
//! parse loop, so the trait requires `Sync`. The implementations here
//! use atomics rather than mutexes to keep contention near zero — the
//! parse worker pool calls `parse_progress` on every file completion,
//! and we don't want that path to acquire any locks.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sink for analysis-pipeline progress events.
///
/// All methods have default no-op bodies so a sink can opt in only to
/// the events it cares about.
pub trait Progress: Sync + Send {
    /// Called once when the filesystem walk starts. Total isn't known
    /// yet — implementations should display a spinner or count-up.
    fn walk_start(&self) {}

    /// Called periodically as the walker emits files. `total` is the
    /// running count of files seen so far (not capped — increases
    /// monotonically until `walk_end`).
    fn walk_progress(&self, _total: usize) {}

    /// Called once the walker has produced the final classified list.
    /// `total_files` is every file the walker yielded (across all
    /// languages, including unsupported); `bytes` is their combined
    /// size for the language-bar.
    fn walk_end(&self, _total_files: usize, _bytes: u64) {}

    /// Called once the parse phase knows its denominator (the number of
    /// source files we're about to feed through tree-sitter).
    fn parse_start(&self, _total_source_files: usize) {}

    /// Called when one source file finishes parsing. `done` is the
    /// running count of completed parses; `total` matches `parse_start`.
    fn parse_progress(&self, _done: usize, _total: usize) {}

    fn parse_end(&self) {}

    /// Called as the orchestrator moves between coarse-grained phases
    /// after the parse loop (graph build, root discovery, tree build,
    /// JSON serialize). Useful for "Building call graph..." messages.
    fn phase(&self, _name: &str) {}

    /// Begin a *counted* phase — i.e. one whose total work units are
    /// known upfront and whose progress can be rendered as a
    /// percentage. The `label` describes the sub-phase ("indexing
    /// symbols", "wiring edges"); `total` is the work-unit
    /// denominator (file count, symbol count, etc.).
    ///
    /// This is the generic equivalent of `parse_start` for every phase
    /// that follows the parse loop. We keep `parse_*` distinct because
    /// it carries unique semantics (the "of N walked" caption and the
    /// final wall-clock elapsed time) that the generic step family
    /// doesn't need.
    fn step_start(&self, _label: &str, _total: usize) {}

    /// Report progress within a counted phase. `done` is the running
    /// count of completed work units; `total` matches `step_start`.
    /// Implementations are expected to throttle their output — this
    /// callback may fire thousands of times per second.
    fn step_progress(&self, _done: usize, _total: usize) {}

    /// Mark the current counted phase complete. Lets terminal sinks
    /// commit the bar to the scrollback with a final newline.
    fn step_end(&self) {}

    /// Report the item currently being processed within a counted
    /// step — typically a file path during parsing, or a symbol name
    /// during tree building. The CLI sink renders this in the
    /// "current item" slot of the active progress bar (last-writer-
    /// wins under concurrent calls from rayon workers). Library
    /// sinks default to a no-op.
    ///
    /// Cheap to call frequently: implementations are expected to
    /// store the value side-channel and only flush on the next bar
    /// redraw, so calling once per parsed file at full speed (~10k
    /// files/sec) is fine.
    fn set_current(&self, _item: &str) {}

    /// Called exactly once at the end of analysis so terminal sinks can
    /// commit the last self-rewriting line with a final newline.
    /// Without this, subsequent stderr writes by the CLI (language
    /// summary, success message) would append to the last phase line.
    fn finish(&self) {}
}

/// Default no-op sink. Library consumers that don't want progress
/// output pass this so the orchestrator can keep calling the trait
/// unconditionally — no `Option<&dyn Progress>` plumbing required.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullProgress;

impl Progress for NullProgress {}

/// Terminal-friendly progress reporter modeled on tqdm / HuggingFace
/// `transformers` / Bun's installer. Two stacked bars (via indicatif
/// `MultiProgress`):
///
///   1. **overall bar** — one line that ticks forward each time a new
///      pipeline phase starts. Shows "Pipeline X/Y" so the user knows
///      how far through the overall scan they are.
///
///   2. **active bar** — the current phase's progress, with the
///      modern tqdm-style stat panel: percentage, count, rate,
///      elapsed, ETA, and the "current item" slot (file path during
///      parse, symbol name during tree build).
///
/// When a phase completes we commit a one-line `✓ <label> in Xs`
/// summary to scrollback via `MultiProgress::suspend(eprintln!(...))`,
/// so the history of completed phases stays visible above the live
/// bars instead of being clobbered.
///
/// TTY handling: indicatif's `ProgressDrawTarget::stderr()` auto-
/// detects whether stderr is a terminal and silently no-ops the bar
/// draws when it isn't. We additionally route the per-phase summary
/// lines through `eprintln!` in that case (via `commit_line` below)
/// so CI / pipe / IDE-terminal contexts still see *something* — just
/// without the flickering live bars.
///
/// Thread-safety: all indicatif handles are internally Arc'd and
/// thread-safe, so the rayon parse loop can call `set_current` /
/// `parse_progress` from any worker without external locking. We
/// only hold a Mutex for the `active` slot pointer because phase
/// transitions need to atomically retire the previous bar and add a
/// new one.
pub struct CliProgress {
    /// Coordinator that draws all child bars in the right Z-order.
    /// Owns its draw target (stderr, auto-hidden if not a TTY).
    mp: MultiProgress,
    /// "Pipeline X/Y" header line. Always at the top of the live
    /// region. We increment its `position` every time a new phase
    /// starts (via `step_start`, `phase`, etc) so the user has a
    /// rough sense of how much of the scan remains.
    overall: ProgressBar,
    /// The bar for the phase currently running. Swapped out on every
    /// `step_start` / `parse_start` / `walk_start` / `phase` call:
    /// previous bar gets committed to scrollback as a `✓` line, new
    /// bar gets `mp.add`'d below the overall.
    active: Mutex<Option<ActivePhase>>,
    /// Index of the next phase we'll start, used to update the
    /// overall bar's position. `AtomicUsize` so we don't need to
    /// hold the active mutex just to bump the counter.
    phase_idx: AtomicUsize,
}

/// Bookkeeping for the live phase bar — the indicatif handle plus
/// when we started it, so `step_end` can compute per-step elapsed.
struct ActivePhase {
    bar: ProgressBar,
    label: String,
    started_at: Instant,
}

/// Hint at how many pipeline phases the overall bar should expect.
///
/// The scan pipeline currently emits roughly:
///   walk(1) + parse(1) + graph-build(4) + entry-decls(1) +
///   root-discovery(1) + tree-build(1) +
///   Report::build attach passes(6) +
///   Summary::build sub-phases(9) +
///   serialize(1) + write(1)
/// ≈ 26 phases for `analyze-root`. The hint affects only the
/// overall bar's percentage display; indicatif tolerates `pos > len`
/// (the bar caps at 100% visually). We pad slightly so future
/// additions don't immediately overflow the visual fill.
const PIPELINE_PHASES_HINT: u64 = 28;

impl Default for CliProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl CliProgress {
    pub fn new() -> Self {
        let mp = MultiProgress::new();
        let overall = mp.add(ProgressBar::new(PIPELINE_PHASES_HINT));
        overall.set_style(
            ProgressStyle::with_template(
                "{prefix:.bold.cyan} {bar:24.cyan/blue} {pos}/{len} phases  {msg:.dim}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█▌ "),
        );
        overall.set_prefix("scan");
        overall.set_message("starting…");
        Self {
            mp,
            overall,
            active: Mutex::new(None),
            phase_idx: AtomicUsize::new(0),
        }
    }

    /// Retire the previous active bar (if any) as a `✓ <label> in Xs`
    /// line in scrollback, then add a fresh `ProgressBar` for the new
    /// phase to the multi-progress and store it as the new active
    /// phase. Returns a handle to the newly-installed bar so callers
    /// can apply phase-specific styles.
    fn enter_phase(&self, label: &str, total: u64, has_count: bool) -> ProgressBar {
        // Finalize the previous active bar (if any) as a scrollback
        // line. The bar itself is dropped — its slot is freed for the
        // new one — but a permanent `✓` summary stays visible.
        if let Some(prev) = self.active.lock().ok().and_then(|mut g| g.take()) {
            let elapsed = prev.started_at.elapsed().as_secs_f64();
            // commit_line handles both TTY (suspend+eprintln) and
            // non-TTY (direct eprintln) so the user always sees the
            // completion record in scrollback.
            self.commit_line(&format!("  ✓ {} ({:.1}s)", prev.label, elapsed));
            prev.bar.finish_and_clear();
        }

        // Bump the overall counter. fetch_add returns the OLD value,
        // so +1 gives the 1-indexed phase number the user reads.
        let n = self.phase_idx.fetch_add(1, Ordering::Relaxed) + 1;
        self.overall.set_position(n as u64);
        self.overall.set_message(label.to_string());

        // Build the new active bar. Two styles:
        //   - has_count=true: counted bar with rate, elapsed, ETA
        //   - has_count=false: spinner for atomic phases (PageRank,
        //     "collecting entries", etc.) where we don't have a
        //     denominator to show progress against.
        let bar = self.mp.add(ProgressBar::new(total));
        if has_count {
            bar.set_style(
                ProgressStyle::with_template(
                    "  {prefix:.green.bold} {bar:24.green/dim} \
                     {pos}/{len} ({percent:>3}%)  \
                     {per_sec:.dim}  \
                     [{elapsed_precise}, ETA {eta}]  \
                     {wide_msg:.cyan}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("█▌ "),
            );
        } else {
            bar.set_style(
                ProgressStyle::with_template(
                    "  {prefix:.green.bold} {spinner:.green}  \
                     [{elapsed_precise}]  {wide_msg:.dim}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            // Keep the spinner animating even without progress
            // updates — atomic phases that take seconds (PageRank on
            // a big graph) should still feel alive.
            bar.enable_steady_tick(Duration::from_millis(120));
        }
        bar.set_prefix(label.to_string());

        // Park the new bar as the active phase for future
        // `step_progress` / `set_current` calls. Lock window is
        // microseconds.
        if let Ok(mut guard) = self.active.lock() {
            *guard = Some(ActivePhase {
                bar: bar.clone(),
                label: label.to_string(),
                started_at: Instant::now(),
            });
        }
        bar
    }

    /// Commit a one-line summary to scrollback. On a TTY we suspend
    /// the multi-progress draw thread for the duration of the
    /// `eprintln!` so the line appears cleanly *above* the live
    /// bars; on a non-TTY (where the bars aren't drawing anyway) we
    /// fall through to a direct `eprintln!` so CI / pipe / IDE-
    /// terminal users still see the completion log.
    fn commit_line(&self, msg: &str) {
        let owned = msg.to_string();
        // `suspend` works on both TTY and hidden targets (no-op
        // suspension on hidden), so the same call is correct in both
        // cases.
        self.mp.suspend(|| {
            eprintln!("{owned}");
        });
    }

    /// Tick the position on the active bar, if any. Used by walk /
    /// parse / step_progress to centralize the "is there an active
    /// bar, is its length sensible, set the position" plumbing.
    fn update_position(&self, done: usize, total: Option<usize>) {
        if let Ok(guard) = self.active.lock() {
            if let Some(p) = guard.as_ref() {
                if let Some(t) = total {
                    // Refresh length each call so a phase whose total
                    // wasn't known at start (e.g. walk) can grow.
                    p.bar.set_length(t as u64);
                }
                p.bar.set_position(done.min(total.unwrap_or(done)) as u64);
            }
        }
    }
}

impl Progress for CliProgress {
    fn walk_start(&self) {
        // No total known yet — use a spinner that ticks per
        // walk_progress callback. We pass total=0 + has_count=false
        // to install the spinner style.
        self.enter_phase("walking filesystem", 0, false);
    }

    fn walk_progress(&self, total: usize) {
        // Stash the running count in the spinner message so the user
        // sees "1024 files" growing — indicatif's spinner doesn't
        // show a percentage, but the message slot is perfect for an
        // ad-hoc counter.
        if let Ok(guard) = self.active.lock() {
            if let Some(p) = guard.as_ref() {
                p.bar.set_message(format!("{total} files seen"));
            }
        }
    }

    fn walk_end(&self, total_files: usize, bytes: u64) {
        // Replace the spinner with a final message; the next
        // enter_phase call will commit it as a "✓" line.
        if let Ok(guard) = self.active.lock() {
            if let Some(p) = guard.as_ref() {
                p.bar.set_message(format!(
                    "{total_files} files, {}",
                    human_bytes(bytes)
                ));
            }
        }
    }

    fn parse_start(&self, total_source_files: usize) {
        self.enter_phase("parsing source", total_source_files as u64, true);
    }

    fn parse_progress(&self, done: usize, total: usize) {
        self.update_position(done, Some(total));
    }

    fn parse_end(&self) {
        // The summary line is emitted by the next `enter_phase` call
        // (which commits the previous bar to scrollback). No action
        // needed here beyond the implicit hand-off.
    }

    fn phase(&self, name: &str) {
        // Atomic phase: spinner only, no counted bar. enter_phase
        // installs the spinner style when has_count=false.
        self.enter_phase(name, 0, false);
    }

    fn step_start(&self, label: &str, total: usize) {
        self.enter_phase(label, total as u64, true);
    }

    fn step_progress(&self, done: usize, total: usize) {
        self.update_position(done, Some(total));
    }

    fn step_end(&self) {
        // Same as parse_end — the summary is committed on the next
        // enter_phase call. This keeps the "completion line" logic
        // in one place (enter_phase) and prevents double-printing.
    }

    fn set_current(&self, item: &str) {
        // tqdm-style "current file" indicator. Concurrent rayon
        // workers race here; last-writer-wins is the desired UX
        // (matches tqdm's `set_postfix` semantics). We truncate to
        // avoid the bar line wrapping on long absolute paths.
        if let Ok(guard) = self.active.lock() {
            if let Some(p) = guard.as_ref() {
                p.bar.set_message(truncate_middle(item, 60));
            }
        }
    }

    fn finish(&self) {
        // Commit the last active bar as a scrollback line, then mark
        // the overall bar done. We don't `finish_and_clear` the
        // overall — its committed final line ("scan 10/10 phases ✓")
        // is useful summary state in the terminal history.
        if let Some(prev) = self.active.lock().ok().and_then(|mut g| g.take()) {
            let elapsed = prev.started_at.elapsed().as_secs_f64();
            self.commit_line(&format!("  ✓ {} ({:.1}s)", prev.label, elapsed));
            prev.bar.finish_and_clear();
        }
        self.overall.finish_with_message("done");
    }
}

/// Truncate a string to `max` chars, replacing the middle with `…`
/// when too long. Keeps both ends visible so a long file path like
/// `/a/b/c/very/long/path/foo.rs` becomes `/a/b/c/very…/foo.rs` —
/// the basename (most informative part) stays visible.
fn truncate_middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    // Keep the last 2/3 of the budget as the "tail" (basename
    // territory) and the first 1/3 as the "head" — biased toward the
    // tail because that's typically what the user wants to read.
    let tail_n = (max - 1) * 2 / 3;
    let head_n = max - 1 - tail_n;
    let head: String = chars.iter().take(head_n).collect();
    let tail: String = chars.iter().skip(chars.len() - tail_n).collect();
    format!("{head}…{tail}")
}

/// Format a byte count as "1.2 MB" / "456 kB". Decimal (kB = 1000)
/// matches how disk-usage tools and GitHub display sizes. Output is
/// always ≤7 chars so the progress line stays bounded.
fn human_bytes(b: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.0} kB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_progress_is_silent_for_every_event() {
        // Mostly a smoke test: confirms every trait method has a
        // default body and NullProgress doesn't panic when the full
        // pipeline calls it.
        let p = NullProgress;
        p.walk_start();
        p.walk_progress(10);
        p.walk_end(10, 1_000);
        p.parse_start(5);
        p.parse_progress(1, 5);
        p.parse_progress(5, 5);
        p.parse_end();
        p.phase("graph");
        // Generic step family — added after `phase` to give counted
        // post-parse phases (graph build, root discovery, tree build)
        // their own percentage bar.
        p.step_start("indexing", 100);
        p.step_progress(50, 100);
        p.step_progress(100, 100);
        p.set_current("foo/bar/baz.rs");
        p.step_end();
        p.finish();
    }

    #[test]
    fn truncate_middle_short_string_is_untouched() {
        // ≤ max → string returned verbatim. Without this we'd insert
        // ellipses into short paths, breaking the file-link display
        // in IDE terminals that auto-linkify "foo.rs:42".
        assert_eq!(truncate_middle("foo.rs", 60), "foo.rs");
        assert_eq!(truncate_middle("", 60), "");
    }

    #[test]
    fn truncate_middle_long_string_keeps_basename() {
        // The tail-biased split must preserve the basename so the
        // user can read which file the active phase is on right now.
        let long = "/Users/me/dev/projects/very/deep/path/components/foo.rs";
        let out = truncate_middle(long, 30);
        assert!(out.contains('…'), "expected ellipsis, got {out:?}");
        assert!(out.ends_with("foo.rs"), "basename should survive, got {out:?}");
        assert_eq!(out.chars().count(), 30, "must hit the budget");
    }

    #[test]
    fn human_bytes_picks_the_right_unit() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_500), "2 kB");
        assert_eq!(human_bytes(1_500_000), "1.5 MB");
        assert_eq!(human_bytes(2_400_000_000), "2.4 GB");
    }
}
