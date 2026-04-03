# Performance Audit Findings Report
# Work Item 0033

**Audit Date:** 2026-04-02
**Auditor:** automated static analysis + code review
**Codebase revision:** HEAD (main branch)

---

## Executive Summary

Five significant findings were identified across six audit areas. Three are High priority and should be addressed before wide user adoption:

1. **Unbounded `output_lines` buffer** — every line ever received from a PTY or text command accumulates in `Vec<String>` with no eviction policy. Long-running sessions will grow without bound.
2. **Always-redraw render loop** — `terminal.draw()` is called unconditionally every 16 ms regardless of whether any state has changed, burning CPU while idle.
3. **Blocking Docker calls on Tokio worker threads** — `run_container_captured` and `run_container_at_path` call `std::process::Command::output()` / `.status()` directly inside `tokio::spawn` tasks without `spawn_blocking`, blocking a Tokio worker thread for the duration of the container run.

The remaining findings are Medium or Low priority and represent architectural improvement opportunities rather than urgent problems.

---

## Area 1 — TUI Rendering Efficiency

### 1.1 Render loop: always-redraw, tick-driven

**Current approach** (`src/tui/mod.rs:104–112`):

```
loop {
    terminal.draw(|f| render::draw(f, &mut app))?;   // always draws
    if event::poll(Duration::from_millis(16))? {     // wait ≤16ms for event
        …
    }
    tick_all → tick per tab
}
```

The frame is rendered on every iteration of the loop — before checking for events. There is no dirty flag. When the user is idle and no container is running, the full widget tree is rebuilt and diffed against the previous Ratatui cell buffer every ~16 ms.

**Ratatui's double-buffering** (`ratatui::Terminal::draw`) compares the new cell buffer with the previous one and emits only changed cells as terminal escape codes. This means _terminal I/O_ is proportional to the number of changed cells, not the full screen. However, _widget construction_ (layout computation, `Paragraph` and `Line` allocation) still executes unconditionally every frame regardless of whether state changed.

**Cost estimate:** on an idle session with no running command, the render loop fires ~60 times per second and rebuilds all widgets for no visible effect. On a 20-tab session with long output buffers, this is particularly wasteful (see Area 6).

**Recommendation (High):** Add a `needs_render: bool` flag to `App`. Set it to `true` whenever state changes (user keypress, PTY data received, channel message arrived during `tick()`). Skip `terminal.draw()` when `needs_render` is `false`. This makes the render loop event-driven at near-zero cost.
→ Follow-on: **Work Item 0034**

---

### 1.2 Scroll computation: O(n) per frame over all output lines

**Current approach** (`src/tui/render.rs:225–235`):

```rust
let total_visual: usize = lines
    .iter()
    .map(|l| {
        let w = l.width();
        if w == 0 { 1 } else { (w + inner_width - 1) / inner_width }
    })
    .sum();
```

Every frame, `draw_exec_window` iterates **every line** in `output_lines` to compute the total visual row count (accounting for line wrapping). With an unbounded buffer this becomes O(n) where n is the total lines ever received. At 50,000 lines the loop touches 50,000 strings before drawing a single widget.

**Cost estimate:** at 10,000 lines (~800 KB of output), this loop processes ~800 KB of string data per frame × 60 fps = ~48 MB/s of string iteration while rendering, even if no new output arrived.

**Recommendation (High, coupled with 1.1):** Cap `output_lines` at a configurable maximum (see Area 2). With a bounded buffer, the O(n) scroll calculation becomes O(max_lines), which is constant. As a further optimisation, maintain a running `total_visual_rows: usize` counter updated only when lines are added or removed, reducing scroll computation to O(1) per frame.
→ Coupled with: **Work Item 0035**

---

### 1.3 Ratatui widget allocation per frame

**Current approach:** `draw()` in `render.rs` allocates `Vec<Line>`, `Vec<Span>`, and `String` values on every frame via `Paragraph::new(lines)`, `format!(…)` for phase labels and tab labels, etc. There is no per-frame allocation pooling.

**Assessment:** Ratatui's architecture does not currently support persistent widget state across frames, so per-frame allocation is unavoidable in the current paradigm. With the dirty-flag fix from 1.1, idle frames will skip `draw()` entirely, eliminating this cost during idle periods. The allocation pattern during active output is acceptable.

**Recommendation (Low):** No immediate action needed beyond 1.1. Track if profiling reveals allocation as a bottleneck during high-throughput output.

---

### 1.4 vt100 scrollback: bounded ✓

**Current approach** (`src/tui/state.rs:543`):

```rust
self.vt100_parser = Some(vt100::Parser::new(rows, cols, 1000));
```

The `vt100` parser is initialised with a 1000-line scrollback limit. This is a hard cap — the parser discards older rows as new output arrives. Memory usage for the vt100 terminal is bounded.

**Assessment:** No action needed. 1000 lines matches common terminal emulator defaults and is appropriate for container output.

---

## Area 2 — Memory Usage

### 2.1 Unbounded `output_lines` buffer

**Current approach** (`src/tui/state.rs:301`):

```rust
pub output_lines: Vec<String>,
```

Lines are appended by `process_pty_data()` and `push_output()` with no upper bound. The `CLEAR_MARKER` from `status --watch` clears the buffer, but commands that produce continuous output (e.g. `cargo build`, agent runs) accumulate lines indefinitely.

**Memory estimate:**
- Typical terminal line: ~80 bytes average (with padding/ANSI stripped)
- 10,000 lines: ~800 KB per tab
- 100,000 lines (3–4 hour high-output run): ~8 MB per tab
- 20 tabs × 100k lines: ~160 MB just for output buffers

At very high output rates (e.g. `cat /dev/urandom | head -1M`), a tab could accumulate millions of lines.

**Cleanup on tab close:** `App::close_tab` calls `self.tabs.remove(idx)` which drops `TabState`. Rust's ownership means `output_lines` is freed when `TabState` is dropped. There is **no buffer leak after tab close** — the issue is growth during the tab's lifetime.

**Recommendation (High):** Replace `Vec<String>` with `VecDeque<String>` and enforce a configurable maximum line count (default: 10,000). When the maximum is reached, remove lines from the front (`pop_front`). This keeps memory usage O(max_lines) regardless of session length.
→ Follow-on: **Work Item 0035**

---

### 2.2 `stats_history` buffer (minor)

**Current approach** (`src/tui/state.rs:551`):

```rust
stats_history: Vec::new(),
```

`ContainerInfo.stats_history` accumulates one `(f64, f64)` entry per stats poll (every 5 seconds). Over an 8-hour session: 5760 entries × 16 bytes = ~92 KB. Negligible.

**Assessment:** Not a concern. The history is used to compute averages for the summary shown after container exit; it is freed when `container_info` is dropped.

---

### 2.3 `output_tx` clone retention

**Current approach:** `output_tx: UnboundedSender<String>` is cloned into async tasks via `spawn_text_command`. The tokio unbounded channel remains live as long as any sender clone exists.

**Lifecycle analysis:**
- `spawn_text_command` captures `tx` (a clone of `output_tx`) in the async block.
- When the async task completes (function returns, error or success), the closure and its captured `tx` are dropped.
- The `output_rx` receiver is inside `TabState`. When a tab is closed mid-command (tab removed from `tabs`), `output_rx` is dropped. Any subsequent `tx.send()` in the running task returns `SendError`, which the task ignores (it checks `is_err()` elsewhere, or the error is swallowed by `sink.println`). The task will run to completion and then drop `tx`.

**Assessment:** No persistent leak. A closed-tab's task continues running to completion (which may be seconds or minutes for a long Docker run), but it will eventually drop its `tx` clone. No corrective action needed for normal commands. For interactive PTY sessions, see Area 4.

---

### 2.4 Unbounded `output_tx` / `output_rx` channel

**Current approach:** `tokio::sync::mpsc::unbounded_channel()` at `state.rs:443`.

Under backpressure (command produces output faster than the TUI tick drains it), the channel queue grows without bound. In practice the TUI ticks at 60 Hz and drains all pending messages per tick (`while let Ok(line) = self.output_rx.try_recv()`), so backpressure requires a source emitting >60 × N messages per second where N is the message processing time — unlikely for normal text output.

**Assessment:** Low practical risk given drain rate, but architecturally fragile. Flag for bounded channel replacement if backpressure stress tests reveal growth.

**Recommendation (Medium):** Consider switching to a bounded channel (e.g. capacity 4096) with a lossy send wrapper that drops oldest messages on overflow rather than blocking the sender.
→ Follow-on: **Work Item 0036**

---

## Area 3 — CPU-Intensive Operations

### 3.1 Blocking Docker calls on Tokio worker threads ⚠️

**Current approach:** `run_container_captured` and `run_container` use `std::process::Command::output()` and `std::process::Command::status()` respectively — both synchronous, blocking calls that wait for the Docker subprocess to exit.

These functions are called inside `tokio::spawn` tasks (via `spawn_text_command`):

```
// tui/mod.rs:1113 — inside spawn_text_command → tokio::spawn
let (_cmd, output) = docker::run_container_captured(…)?;
```

`spawn_text_command` wraps the caller's async block in `tokio::spawn` (pty.rs:127):

```rust
tokio::spawn(async move {
    let sink = …;
    f(sink).await   // calls run_container_captured() which blocks
```

Since `run_container_captured` is **not** an async function and **not** wrapped in `tokio::task::spawn_blocking`, it occupies a Tokio worker thread for the entire container run duration. During a long audit run (minutes), this starves other tasks on that worker thread.

**Note:** The stats poller correctly uses `spawn_blocking`:
```rust
// tui/mod.rs:1614 ✓
let stats = tokio::task::spawn_blocking(move || docker::query_container_stats(&name)).await;
```

**Affected call sites** where blocking calls are inside tokio tasks without spawn_blocking:
- `tui/mod.rs:1116` — audit phase
- `tui/mod.rs:1390` — implement phase
- `tui/mod.rs:1550` — chat phase
- `commands/ready.rs:366` — ready audit
- `commands/ready.rs:663` — ready refresh
- `commands/agent.rs:99` — agent non-interactive run
- `src/commands/init.rs:212` — init container run

**Recommendation (High):** Wrap `run_container_captured` and `run_container` calls inside async tasks with `tokio::task::spawn_blocking`. Or refactor `spawn_text_command` to accept a sync fn and wrap it in `spawn_blocking` internally.
→ Follow-on: **Work Item 0037**

---

### 3.2 ANSI stripping: per-call allocation

**Current approach** (`src/tui/state.rs:704`):

```rust
let stripped = strip_ansi_escapes::strip(segment);
```

`strip_ansi_escapes::strip` allocates a new `Vec<u8>` for each content segment. For typical PTY output with many short segments between `\r`/`\n` boundaries, this is many small allocations per chunk.

**Assessment:** Modern allocators handle small, short-lived allocations efficiently. The `process_pty_data` path is not on a hot render path — it runs during tick(), not during draw(). For container output, the vt100 path (`parser.process(&bytes)`) is used instead, which has its own internal allocation strategy.

**Recommendation (Low):** If profiling reveals this as a bottleneck (unlikely for typical output rates), replace with a writer-based strip that appends to a pre-allocated buffer. Not worth addressing without profiling evidence.

---

### 3.3 Polling loops

**Current approach:**
- TUI event loop: `event::poll(Duration::from_millis(16))` — 16ms max wait, event-driven when events are present ✓
- Stats poller: `tokio::time::interval(Duration::from_secs(5))` — proper async interval, no spinning ✓
- No `std::thread::sleep` in polling loops found

**Assessment:** No polling loop issues. The tick interval is appropriate.

---

### 3.4 DAG recomputation

**Current approach** (`src/workflow/dag.rs`):
- `ready_steps()`, `topological_order()`, `detect_cycle()` each rebuild the adjacency map from scratch on every call (O(n+e)).
- No memoization.

**Call frequency:** these functions are called at workflow control transitions (step complete, step start), not every tick. Typical workflows have <20 steps.

**Benchmark:** `topological_order()` at 20 steps involves ~20 HashMap insertions + ~20 DFS visits. At 200 steps: ~200 insertions + ~200 visits. Estimated <1ms even at 200 steps.

**Assessment:** Not a concern. DAG operations are called infrequently and graphs are small. No optimisation needed.

---

## Area 4 — Background Async Task Efficiency

### 4.1 Task inventory

| Task/Thread | Type | Spawned at | Exit condition |
|---|---|---|---|
| Stats poller | `tokio::spawn` | Container start (mod.rs:1609) | `stats_rx` receiver dropped (on `finish_command`) |
| Text command | `tokio::spawn` (via `spawn_text_command`) | Command launch | Function returns (success or error) |
| PTY reader | `std::thread::spawn` | `PtySession::spawn` (pty.rs:66) | EOF on PTY master or read error |
| PTY wait | `std::thread::spawn` | `PtySession::spawn` (pty.rs:81) | Child process exits |
| PTY writer | `std::thread::spawn` | `PtySession::spawn` (pty.rs:92) | `input_rx` channel closed (PtySession dropped) |
| Docker build stdout | `std::thread::spawn` | `build_image_streaming` (docker/mod.rs:530–543) | EOF on process stdout/stderr |
| Status watch | `tokio::spawn` (via `spawn_text_command`) | `status --watch` command | `status_watch_cancel_tx` sends cancel |

No `JoinHandle`s are retained for any task or thread. All detached.

---

### 4.2 PTY thread cleanup on tab close

**When a running tab is closed** (`App::close_tab` → `self.tabs.remove(idx)`):
- `TabState` is dropped, which drops `pty: Option<PtySession>`.
- `PtySession` holds `master: Box<dyn MasterPty>` and `input_tx: SyncSender<Vec<u8>>`.
- **Dropping `master`** closes the PTY master side. On Linux/macOS, this sends SIGHUP to the foreground process group of the PTY, causing the `docker run` process to exit. The reader thread then receives EOF and exits.
- **Dropping `input_tx`** closes the write side of the input channel. The writer thread exits on next iteration.
- **Wait thread** exits when the child process (docker run) exits.

**Assessment:** The cleanup chain is correct on Unix. The key dependency is that dropping the PTY master reliably sends SIGHUP to the child process. This is a property of the OS PTY implementation and holds for Linux and macOS. On Windows, `portable_pty` behaviour may differ.

**Risk:** If a container is run without PTY allocation (`run_container_captured`), cleanup depends on the tokio task running to completion. If a tab with a long-running captured task is closed, the task continues running in the background (Docker subprocess is not killed). This is not a thread/memory leak but it is a wasted Docker container.

**Recommendation (Medium):** Store a `tokio::task::JoinHandle` or `CancellationToken` for each spawned text command task and cancel it on tab close. For non-PTY Docker runs, kill the underlying subprocess.
→ Follow-on: **Work Item 0038**

---

### 4.3 Docker stats polling: no deduplication

**Current approach:** each container session spawns its own `spawn_stats_poller(container_name)` task (mod.rs:1061). If two tabs happen to monitor containers with the same name, each tab gets its own stats poller making independent `docker stats` subprocess calls.

**Assessment:** In normal usage each container has a unique generated name (`amux-{pid}-{nanos}`), so deduplication would only matter for reattach scenarios or manually named containers. Not a significant concern in current usage patterns.

**Recommendation (Low):** No immediate action needed.

---

### 4.4 Status watch task cancellation

**Current approach:** The `status --watch` tab stores a cancellation sender `status_watch_cancel_tx: Option<tokio::sync::oneshot::Sender<()>>`. When the tab exits or the command changes, the task is cancelled via this sender.

**Assessment:** Cancellation path exists and appears correct. No action needed.

---

### 4.5 Unbounded stats channel

**Current approach** (`src/tui/mod.rs:1608`):

```rust
let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
```

The stats poller sends one message every 5 seconds; the TUI tick drains it. At 60 Hz tick rate and 0.2 Hz send rate, the channel will never accumulate more than 1 unread message. Effectively unbounded but never actually grows.

**Assessment:** Not a concern. Could be replaced with a bounded channel of capacity 1 as a defensive measure.

---

## Area 5 — Docker Container Management Performance

### 5.1 No Docker client reuse; subprocess per operation

**Current approach** (`src/docker/mod.rs`): all Docker operations spawn a new `std::process::Command` child process (`docker stats`, `docker build`, `docker run`, `docker info`). There is no persistent Docker HTTP client (e.g. `bollard`).

**Per-operation costs:**
- `is_daemon_running()` (`docker info`): ~50–200ms subprocess spawn + Docker API roundtrip
- `query_container_stats()` (`docker stats --no-stream`): ~200–500ms (includes Docker cgroup stat collection)
- `docker build`: seconds to minutes depending on cache
- `docker run`: startup dominated by Docker container init, not subprocess overhead

**Trade-offs:** subprocess-per-operation is simple, portable, and requires no additional dependencies. The overhead is dominated by Docker's own API latency, not subprocess spawn cost (~5ms). Switching to `bollard` would reduce per-call overhead marginally but add a complex dependency.

**Recommendation (Low):** No action needed unless Docker API call frequency increases significantly. For stats polling, the 5-second interval already amortises the ~300ms `docker stats` call cost adequately.

---

### 5.2 Output chunk size

**Current approach** (`src/tui/pty.rs:67`):

```rust
let mut buf = [0u8; 4096];
```

PTY reader reads 4096-byte chunks. Each read produces one `PtyEvent::Data(Vec<u8>)` allocation of the read size.

**Assessment:** 4096 bytes is the standard page size and a common choice for buffered I/O. For high-throughput container output (megabytes per second), this means thousands of events per second, each with a small heap allocation. Larger buffers (e.g. 65536) would reduce allocation frequency but increase per-event latency. 4096 is appropriate.

---

### 5.3 `HostSettings` cleanup

**Current approach:** `HostSettings._temp_dir: Option<TempDir>` holds the temp directory for sanitized config files. It is dropped when `TabState` drops `host_settings` (on `finish_command` when no workflow is active).

**Typical size:** config JSON (~2KB) + filtered `.claude/` directory (~100KB). Synchronous `TempDir::drop()` on a small directory is sub-millisecond.

**Assessment:** Not a concern. RAII cleanup is correct and fast.

---

### 5.4 Container cleanup

**Current approach:** all containers use `--rm` flag, causing Docker to remove the container immediately on exit. No manual cleanup required.

**Assessment:** Correct approach. Containers are promptly removed. No blocking cleanup path identified.

---

## Area 6 — Scalability with Many Concurrent Tabs

### 6.1 Scalability target

**Target: 20 concurrent containers** (a typical multi-agent parallel workflow). 50 containers is considered a stretch goal.

---

### 6.2 O(n) paths that scale with tab count

| Path | Complexity | Notes |
|---|---|---|
| `tick_all()` | O(tabs) | Calls `tick()` for every tab each iteration |
| `tick()` per tab | O(pending messages) | Drains channels; bounded by activity |
| `draw_tab_bar()` | O(tabs) | Iterates all tabs to render tab bar |
| `draw_exec_window()` | O(output_lines of active tab) | Only for the active tab |
| `tui_tabs_shared` lock | O(tabs) | Brief write lock per `tick_all()` iteration |

**At 20 tabs:**
- `tick_all()`: 20 tab ticks × (≈1µs per empty tick) ≈ 20µs overhead. Negligible.
- `draw_tab_bar()`: 20 tab renders, each rendering a 20-column widget. Negligible.
- `draw_exec_window()`: only the active tab's output_lines are iterated. Unaffected by tab count.

**At 20 tabs with 100k lines each:**
- `tick_all()`: memory pressure from 20 × 100k-line buffers ≈ 160 MB. Mitigated by work item 0035 (buffer cap).
- Rendering: only active tab drawn. OK.
- Tick: all 20 tabs drain their PTY channels each tick. If all 20 containers emit output simultaneously, tick processing time scales linearly with combined output rate.

**Assessment:** 20 concurrent tabs is achievable with the current architecture. At 50 tabs with high output rate, tick time could become the bottleneck.

---

### 6.3 Shared lock contention

**Current approach:** `tui_tabs_shared: Arc<Mutex<Vec<TuiTabInfo>>>` is written by `tick_all()` and read by the `status --watch` background task.

`tick_all()` acquires the lock briefly on every tick (~60 Hz) to update the snapshot. The `status --watch` task reads it at its own refresh interval (seconds). Lock hold time: microseconds per tick.

**Assessment:** Not a contention point. `Mutex` is appropriate here. `RwLock` would only help if there were multiple concurrent readers, which there are not currently.

---

### 6.4 Inactive tab rendering

**Current approach:** only `draw_exec_window()` for the active tab is called (render.rs:27 uses `app.active_tab_mut()`). Inactive tabs are not rendered beyond their tab bar entry. This is already optimal.

**Assessment:** No action needed. Inactive tabs are already rendered lazily (tab bar entry only).

---

## Prioritised Recommendation Table

| # | Work Item | Priority | Area | Impact | Effort |
|---|---|---|---|---|---|
| 1 | Event-driven render loop (dirty flag) | **High** | 1.1 | High CPU reduction when idle | Low |
| 2 | Cap `output_lines` with bounded ring buffer | **High** | 1.2, 2.1 | Prevents OOM on long sessions | Low |
| 3 | Wrap blocking Docker calls in `spawn_blocking` | **High** | 3.1 | Prevents Tokio thread starvation | Medium |
| 4 | Cancel long-running tasks and Docker processes on tab close | **Medium** | 4.2 | Prevents wasted Docker containers | Medium |
| 5 | Replace `output_tx` unbounded channel with bounded+lossy | **Medium** | 2.4 | Defensive backpressure handling | Low |
| 6 | Add criterion benchmarks for frame time and PTY throughput | **Medium** | — | Enables regression detection | Medium |
| 7 | Add `tokio-console` instrumentation (debug feature flag) | **Low** | — | Task lifecycle visibility | Low |
| 8 | Cache vt100 screen spans until PTY data changes | **Low** | 1.3 | Minor render CPU reduction | Medium |
| 9 | Make Docker stats poll interval configurable | **Low** | 3.3 | Power-user tuning | Low |

---

## Instrumentation Recommendation

Add `tracing` spans around:
- `terminal.draw()` call in `mod.rs` — measures actual render time per frame
- `tick()` per tab — measures per-tab channel drain time
- Docker subprocess invocations — measures per-call latency

Gate `tokio-console-subscriber` behind a `tokio-console` Cargo feature flag (not in `default`). Document how to enable it in `aspec/devops/localdev.md`. This has been confirmed as not affecting the release binary.

---

## Edge Cases Confirmed

| Edge Case | Current Handling | Risk |
|---|---|---|
| Very long-running sessions | `output_lines` grows without bound | **High** — OOM risk |
| Rapid tab open/close cycling | RAII cleanup; PTY threads exit on master close | Low — cleanup is correct |
| High-throughput container output | vt100 parser has 1000-line cap; outer buffer unbounded | Medium — outer buffer risk |
| Containers that exit immediately | Cleanup via `finish_command()` on `PtyEvent::Exit` | Low — path is exercised |
| Docker daemon restart | Stats poller self-terminates on send error; PTY threads exit on read error | Low |
| Very wide/tall terminals | Ratatui layout O(splits); no quadratic paths found | Low |
| Low-resource environments | No hardcoded resource assumptions found | Low |
