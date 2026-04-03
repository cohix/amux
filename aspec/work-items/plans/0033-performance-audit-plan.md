# Implementation Plan: 0033 – Performance Audit

## Overview

This plan organises the investigation into a set of phases, each targeting one of the six audit areas from the work item. Every phase produces concrete, measurable findings and ends with one or more draft recommendations to be turned into follow-on work items.

**Output of this work item:** a written audit report (`aspec/work-items/plans/0033-performance-audit-findings.md`) and a set of new work items (one per distinct concern, filed under `aspec/work-items/`). No production code changes are made.

---

## Phase 0 – Tooling Setup (prerequisites)

**Goal:** put the measurement infrastructure in place before starting any audit work.

### 0.1 Add dev-only profiling dependencies

Add the following to `[dev-dependencies]` and/or `[profile.bench]` in `Cargo.toml` (none affect the release binary):

| Crate | Purpose |
|---|---|
| `criterion` (with `html_reports`) | Frame-time and throughput micro-benchmarks |
| `tokio-console-subscriber` (feature-gated) | Task lifetime visualisation |
| `dhat` or `dhat-heap` | Heap allocation profiling (opt-in) |

Also verify that `cargo flamegraph` and `heaptrack` are available in the dev environment (document the install steps in the findings report; no code changes needed).

Gate `tokio-console-subscriber` behind a Cargo feature flag (e.g., `tokio-console`) so it is never compiled into release builds. The feature must not be listed in `default`.

### 0.2 Establish a benchmark harness

Create `benches/` directory with three benchmark files (stubs at this phase, populated during audit phases):

- `benches/render.rs` – measures frame draw time at N tabs
- `benches/pty_parse.rs` – measures PTY byte throughput via `process_pty_data()`
- `benches/dag.rs` – measures DAG topological sort at varying workflow sizes

Run `cargo bench` once to confirm everything compiles and produces baseline numbers (even with trivial inputs). Record the baseline numbers in the findings report.

---

## Phase 1 – TUI Rendering Efficiency (`src/tui/render.rs`, `src/tui/mod.rs`)

**Goal:** determine whether the render loop wastes CPU and identify the dominant cost per frame.

### 1.1 Full-frame vs dirty-region analysis

**Current state (from codebase inspection):**
- `render.rs:14` – `draw()` redraws every widget unconditionally every tick.
- `mod.rs` event loop polls with a 16 ms timeout (≈60 Hz tick), unconditionally calling `terminal.draw(|f| draw(f, app))` on every iteration regardless of whether any state changed.
- No dirty flag or event-driven render trigger exists.

**Investigation steps:**
1. Read `src/tui/mod.rs` carefully to confirm whether there is any condition guarding the `terminal.draw()` call. If none, confirm "always-redraw" finding.
2. Measure the cost of a single `draw()` call by wrapping it in a `criterion` benchmark (`benches/render.rs`) with a fixed `App` containing 1, 5, and 20 tabs. Record mean frame time and allocations per frame.
3. Check whether Ratatui's `Terminal::draw()` itself does double-buffered diffing (it does — Ratatui compares old/new cell buffers before emitting terminal escape codes). Document this as a partial mitigation, but note that widget construction and layout calculation still runs every frame even if no output is emitted.

**Findings to document:**
- Frame time at 1 / 5 / 20 tabs
- Whether the event loop has any idle guard (expected: none)
- Whether Ratatui's diffing eliminates redundant terminal writes (expected: yes, but CPU cost of widget tree construction remains)

**Recommendation draft:**
> Add a `needs_render: bool` flag to `App`. Set it to `true` whenever state changes (input event, PTY output, tick with new data). Skip `terminal.draw()` when `needs_render` is false. This converts the render loop from poll-driven to event-driven without changing the tick architecture. Priority: **High**.

---

### 1.2 Scroll computation cost

**Current state:**
- `render.rs:225–244` – `draw_exec_window()` iterates all lines in `output_lines` each frame to compute total visual height for scroll offset rendering (O(n) per frame where n = line count).
- `output_lines: Vec<String>` in `state.rs:301` is unbounded.

**Investigation steps:**
1. Read the exact scroll calculation in `render.rs`. Confirm whether the loop visits every line or only the visible window.
2. Run a benchmark: populate `output_lines` with 10k, 50k, and 100k lines and measure frame time.
3. Measure memory footprint of `output_lines` by checking string sizes (e.g., average PTY line length × line count).

**Findings to document:**
- Confirm O(n) vs O(visible) rendering
- Frame time degradation vs line count
- Memory footprint at session scale

**Recommendation draft:**
> Cap `output_lines` at a configurable maximum (default 10,000 lines, matching common terminal emulators). When the cap is reached, drop the oldest N lines. Also maintain a running `total_visual_rows` counter so scroll offset rendering is O(1). Priority: **High**.

---

### 1.3 vt100 per-cell rendering cost

**Current state:**
- `render.rs:316–368` – `render_vt100_screen()` iterates every cell in the visible vt100 screen area per frame. Each cell: color lookup, modifier assembly, span construction.
- `vt100::Parser` is constructed with 1000-line scrollback (`state.rs:543`).

**Investigation steps:**
1. Measure `render_vt100_screen()` in isolation using `criterion` at typical terminal sizes (80×24, 220×50).
2. Determine whether `vt100::Parser`'s cell grid is allocated once and mutated in-place, or rebuilt on each `process()` call.
3. Check whether the 1000-line scrollback is appropriate or if it can be reduced without visible impact.

**Findings to document:**
- Per-frame cost of vt100 screen rendering vs terminal size
- Whether cell grid allocation is bounded

**Recommendation draft:**
> Cache vt100 screen cells as a `Vec<Span>` and invalidate only when new PTY data arrives. This avoids per-cell conversion on idle frames. Priority: **Medium**.

---

## Phase 2 – Memory Usage

**Goal:** identify sources of unbounded memory growth and retained allocations.

### 2.1 Output buffer growth

**Current state:**
- `state.rs:301` – `output_lines: Vec<String>` has no size limit.
- Lines are appended by `process_pty_data()` and drained from `output_rx` in `tick()`.
- **No eviction policy exists.**

**Investigation steps:**
1. Instrument `tick()` to log `output_lines.len()` at regular intervals during a long-running session (>30 min, high-output container). Use `tracing::debug!` (already presumably in use).
2. Use `heaptrack` or `dhat` on a session where a container emits continuous output to measure heap growth over time.
3. Confirm there is no cap by searching the codebase for any `.truncate()` or `.drain()` on `output_lines`.

**Findings to document:**
- Confirmed unbounded growth
- Measured growth rate (bytes/min) for typical container output
- Memory at 10 min, 30 min, 60 min

**Recommendation draft:**
> Introduce a ring-buffer (`VecDeque<String>`) for `output_lines` with a configurable maximum (default: 10,000 lines). Drop oldest entries on overflow. Priority: **High**.

---

### 2.2 Buffer retention after tab close

**Current state:**
- `TabState` is removed from `App.tabs: Vec<TabState>` when a tab is closed.
- Rust ownership means the `Vec<String>` inside `TabState` is dropped when `TabState` is dropped.
- However: `output_tx: UnboundedSender<String>` is passed (cloned) into async tasks or threads. If any such task holds a live clone of `output_tx`, the `output_rx` half inside `TabState` will not be dropped until both sides are released.

**Investigation steps:**
1. Trace every clone of `output_tx` across `state.rs`, `commands/`, and `docker/`. Document where each clone lives and what owns it.
2. Verify that closing a tab cancels/drops all holders of `output_tx` clones. Look for any `tokio::spawn` that captures `output_tx` without a cancellation path.
3. Add a drop-test: write a unit test that creates a `TabState`, sends data, drops the tab, and asserts the channel is closed.

**Findings to document:**
- All `output_tx` clone sites
- Whether tab close reliably drops all of them
- Any confirmed or suspected retention

**Recommendation draft:**
> Audit every `output_tx` clone site and ensure the associated task/thread is cancelled on tab close. Add a unit test that asserts `output_rx.is_closed()` after tab drop. Priority: **Medium**.

---

### 2.3 Unbounded channels

**Current state:**
- `output_rx / output_tx` – `tokio::sync::mpsc::unbounded_channel()` (state.rs). Unbounded.
- `stats_rx` – Also unbounded receiver.
- PTY event channel in `pty.rs` – `std::sync::mpsc::sync_channel(256)` — **bounded** (good).
- PTY input channel – `std::sync::mpsc::sync_channel(64)` — **bounded** (good).

**Investigation steps:**
1. Find all `unbounded_channel()` calls via `grep -rn "unbounded_channel"`.
2. For each, assess worst-case sender rate and whether the receiver keeps up.
3. Specifically: if a container emits output faster than the TUI tick drains `output_rx`, the channel will grow. Measure drain rate vs emission rate under a synthetic stress test (cat a large file inside the container).

**Findings to document:**
- All unbounded channel locations
- Worst-case backpressure scenario
- Whether bounded channels would cause deadlocks (assess carefully)

**Recommendation draft:**
> Replace `output_rx/output_tx` with a bounded channel (e.g., 4096 messages). If the channel is full, drop the oldest message rather than blocking the sender (implement a lossy sender wrapper). Priority: **Medium**.

---

## Phase 3 – CPU-Intensive Operations

**Goal:** find synchronous blocking calls on async executors and CPU-heavy hot paths.

### 3.1 Blocking calls in async context

**Current state:**
- `docker/mod.rs` – `run_container_captured()` (line 604) and `run_container()` (line 681) both call `.output()` / `.status()` which block the calling thread until the container exits. If called from an async context without `spawn_blocking`, this blocks the Tokio thread.
- `agent.rs` – calls Docker functions directly from async context; needs verification.

**Investigation steps:**
1. Search for `.output()`, `.status()`, `.wait()`, `std::thread::sleep`, and `std::fs::read*` in all `async fn` bodies.
2. For each hit, determine whether the call is wrapped in `tokio::task::spawn_blocking`. If not, and the call can block for >1 ms, flag it.
3. Check whether any Docker subprocess call is made from the TUI event loop's async task (which would stall the entire loop).

**Findings to document:**
- All blocking calls in async context
- Estimated block duration per call (sub-ms OK; >1 ms problematic)

**Recommendation draft:**
> Wrap all `std::process::Command::output()` / `.status()` calls in `tokio::task::spawn_blocking`. Priority: **High** for any call on the TUI task; **Medium** otherwise.

---

### 3.2 ANSI/VT escape sequence parsing overhead

**Current state:**
- Non-container output path: `process_pty_data()` calls `strip_ansi_escapes::strip()` per segment (each call re-allocates a `Vec<u8>`).
- Container output path: `vt100::Parser::process(&bytes)` — incremental, stateful; typically efficient.
- Reader thread reads 4096-byte chunks and sends them as `PtyEvent::Data(Vec<u8>)` — one allocation per chunk.

**Investigation steps:**
1. Benchmark `process_pty_data()` using `criterion` by replaying captured PTY output (e.g., `cargo build` output, 10 MB). Measure throughput (MB/s).
2. Profile allocation count using `dhat`: count allocations per call to `strip_ansi_escapes::strip()`.
3. Check whether `strip_ansi_escapes` offers an in-place or writer-based API to avoid allocation.

**Findings to document:**
- Throughput of `process_pty_data()` in MB/s
- Allocation count per call
- Whether strip_ansi_escapes allocation is avoidable

**Recommendation draft:**
> If throughput is below ~50 MB/s or allocation count is high, replace `strip_ansi_escapes::strip()` with an in-place variant or a writer that appends to a pre-allocated buffer. Priority: **Low** (likely not a bottleneck for typical use, but flag if stress test shows starvation).

---

### 3.3 Polling loops

**Current state:**
- `mod.rs` tick loop polls at 16 ms unconditionally.
- `stats_rx` (Docker stats) — background task queries `docker stats --no-stream` periodically; frequency unknown.

**Investigation steps:**
1. Find all `tokio::time::sleep` or `std::thread::sleep` inside loops. Measure the sleep duration.
2. For Docker stats polling, find where the interval is set and whether it is configurable.
3. Check whether stats polling uses `tokio::time::interval` (preferred) or `sleep`-in-loop.

**Findings to document:**
- All polling loop locations and intervals
- Whether stats polling can be reduced (e.g., 2s instead of 500ms)

**Recommendation draft:**
> Make the Docker stats poll interval configurable (default: 2s). Replace any `sleep`-in-loop patterns with `tokio::time::interval`. Priority: **Low**.

---

### 3.4 DAG recomputation

**Current state:**
- `dag.rs` – `ready_steps()`, `topological_order()`, `detect_cycle()` rebuild the adjacency map and run full traversal on every call.
- Workflows are small (<100 steps), so this is unlikely to be a bottleneck in practice.

**Investigation steps:**
1. Benchmark `topological_order()` at 10, 50, and 200 steps using `criterion` (`benches/dag.rs`).
2. Confirm that the only caller is the workflow state machine and determine call frequency.

**Findings to document:**
- Latency at realistic workflow sizes
- Call frequency during a session

**Recommendation draft:**
> If latency is <1 ms at 200 steps, no action needed. Document as **Low** priority or **Not a concern**. If called very frequently (e.g., every tick), cache the result and invalidate only on state change.

---

## Phase 4 – Background Async Task Efficiency

**Goal:** confirm tasks are bounded and correctly cancelled; prevent orphaned tasks.

### 4.1 Task inventory

**Investigation steps:**
1. Search for all `tokio::spawn`, `std::thread::spawn`, and `tokio::task::spawn_blocking` calls across the codebase.
2. For each spawned task/thread, document:
   - What it does
   - What owns the `JoinHandle` (if any)
   - What triggers cancellation (e.g., channel close, explicit abort, drop of handle)
3. Pay special attention to tasks in `commands/agent.rs` and `tui/state.rs`.

**Current state (partial, from inspection):**
- `pty.rs` – 3 `std::thread::spawn` per PTY session (reader, wait, writer). No stored `JoinHandle`; threads detach. They will exit when: reader gets EOF (process exits), wait thread returns, writer channel closes.
- `output_tx` clones passed to Docker command threads — need to verify these threads are bounded.

**Findings to document:**
- Full task/thread inventory
- Any detached tasks without a clear exit condition
- Any `JoinHandle` that is dropped (cancels the task) vs awaited

**Recommendation draft:**
> For any task where the exit condition is unclear, add explicit cancellation via `CancellationToken` (from `tokio-util`). Store handles in `TabState` and cancel/join them on tab close. Priority: **Medium**.

---

### 4.2 Task cancellation on tab close

**Investigation steps:**
1. Trace the tab-close code path (find where a tab is removed from `App.tabs`).
2. Confirm that all channels and tasks associated with the tab are dropped/cancelled.
3. Write a test: open a tab, start a PTY session, close the tab, assert all threads have exited within 500 ms.

**Findings to document:**
- Whether tab close reliably cancels PTY threads
- Whether container processes are also killed (via `docker kill` or SIGTERM to child `docker run` process)

**Recommendation draft:**
> If PTY threads are not reliably cancelled, add explicit cleanup: kill the child process on tab close, drop the PTY master (which closes the reader), and join/detach writer thread. Priority: **High** if leak confirmed.

---

### 4.3 Duplicate Docker subscriptions

**Current state:**
- Each tab runs its own `docker run --rm` process.
- Stats polling: each tab with a running container spawns an independent `docker stats` poll task (suspected; verify).

**Investigation steps:**
1. Confirm whether multiple tabs monitoring the same container (same `container_name`) each spawn a separate stats poll.
2. Check whether `tui_tabs_shared: Arc<Mutex<...>>` is used for deduplication.

**Findings to document:**
- Whether duplicate polling occurs
- Whether it is possible to share a single stats stream across tabs watching the same container

**Recommendation draft:**
> If duplicate polling is confirmed, consider a shared stats poller keyed by container name. Priority: **Low** (most users have one container per tab).

---

## Phase 5 – Docker Container Management Performance

**Goal:** assess Docker API call overhead and identify optimisation opportunities.

### 5.1 Client reuse and subprocess overhead

**Current state:**
- `docker/mod.rs` – every operation spawns a new `docker` subprocess (`std::process::Command`). No persistent Docker HTTP client.
- Pro: simple, no dependency on `bollard` or similar. Con: subprocess spawn overhead per call.

**Investigation steps:**
1. Measure the latency of `docker stats --no-stream` (stats poll), `docker ps --filter name=X` (if used), and `docker run` start-up using `criterion` or manual timing.
2. Count total Docker subprocess invocations during a typical session (instrument with `tracing`).
3. Assess whether moving to a `bollard`-based HTTP client would reduce per-call overhead significantly.

**Findings to document:**
- Per-call latency for each Docker operation
- Total call count per session
- Estimated savings from HTTP client approach

**Recommendation draft:**
> If per-call overhead is >50 ms and call frequency is high, introduce `bollard` as an optional async Docker client. If overhead is acceptable, document as **Low** priority.

---

### 5.2 Container startup latency

**Investigation steps:**
1. Time the sequence from "user presses enter on a tab" to "first PTY output appears". Break it down into: Docker image check, `docker run` invocation, container entrypoint start.
2. Identify any sequential steps (e.g., pull check then run) that could be parallelised or cached.

**Findings to document:**
- End-to-end startup latency
- Whether image existence check is redundant (if image is always pre-built)

**Recommendation draft:**
> Cache the result of image existence checks within a session. If startup is dominated by `docker run` cold-start, document as not actionable without architectural change.

---

### 5.3 Container output streaming chunk size

**Current state:**
- `pty.rs` reader thread reads 4096-byte chunks from PTY master.
- This is reasonable for most use cases.

**Investigation steps:**
1. Confirm chunk size in `pty.rs` (expected: 4096).
2. Under high-throughput output (e.g., `yes | head -1000000`), measure whether small chunks cause high syscall overhead or whether 4096 is sufficient.
3. Check whether the PTY reader spins (busy-loops) or blocks on `read()`.

**Findings to document:**
- Confirmed chunk size
- Syscall rate under high-throughput output

**Recommendation draft:**
> If syscall overhead is measurable, consider increasing chunk size to 65536. PTY reader is already blocking-on-read (not spinning), so this is low priority unless stress test shows starvation.

---

### 5.4 Cleanup blocking the UI

**Current state:**
- `--rm` flag on all containers means Docker cleans up containers automatically.
- `HostSettings._temp_dir` is dropped when `TabState` is dropped (RAII). `TempDir::drop()` performs synchronous filesystem cleanup. If the temp dir is large, this could block the TUI event loop.

**Investigation steps:**
1. Measure how large `HostSettings._temp_dir` typically is (config files only — expected to be very small: a few KB).
2. Confirm that `TabState::drop()` is not called from the TUI tick task directly.
3. Check whether any cleanup logic blocks on `docker wait` or a Docker API call.

**Findings to document:**
- Size of temp dir on drop
- Whether drop is synchronous and on what thread/task
- Any blocking cleanup in the event loop path

**Recommendation draft:**
> If `TempDir` drop is measurably slow (unlikely for config-only dirs), move cleanup to `spawn_blocking`. Otherwise, document as **Not a concern**.

---

## Phase 6 – Scalability with Many Concurrent Tabs

**Goal:** define a scalability target and identify O(n) degradation paths.

### 6.1 Define "many tabs"

Based on typical agent workflows, establish the scalability target as **20 concurrent tabs** (containers). Document reasoning in the findings report.

### 6.2 O(n) render paths

**Investigation steps:**
1. In `render.rs`, identify every loop or iterator that scales with tab count (e.g., drawing the tab bar, iterating `App.tabs` for rendering).
2. Measure frame time at 1, 5, 10, and 20 tabs using `benches/render.rs`.
3. Identify any per-tab O(n) operation inside the tick loop (e.g., `tick()` called for every tab, each O(m) where m = output lines).

**Findings to document:**
- Frame time vs tab count
- Whether degradation is linear or worse
- Whether inactive tabs can short-circuit rendering

**Recommendation draft:**
> If frame time at 20 tabs exceeds 16 ms (60 Hz budget), add a "visible tab only" optimisation: only call the full render path for the active tab; render inactive tabs as a single line in the tab bar. Priority: **Medium**.

---

### 6.3 Shared lock contention

**Current state:**
- `tui_tabs_shared: Arc<Mutex<Vec<TuiTabInfo>>>` is locked by the TUI event loop (write) and the `status --watch` command (read).

**Investigation steps:**
1. Find all `.lock()` calls on `tui_tabs_shared` and measure lock hold time.
2. Assess whether `RwLock` would be more appropriate (many readers, infrequent writers).
3. Check whether any other `Mutex`/`RwLock` exists in the hot path.

**Findings to document:**
- Lock contention potential at high tab count
- Whether `RwLock` is warranted

**Recommendation draft:**
> Replace `Mutex<Vec<TuiTabInfo>>` with `RwLock<Vec<TuiTabInfo>>`. Priority: **Low** (only one writer and one reader in current architecture).

---

## Phase 7 – Benchmarks and Stress Tests

**Goal:** establish reproducible baselines and regression guards.

### 7.1 Benchmarks to implement (`benches/`)

| File | Benchmark | Input range |
|---|---|---|
| `benches/render.rs` | Frame draw time | 1, 5, 10, 20 tabs; 100, 1k, 10k lines |
| `benches/pty_parse.rs` | `process_pty_data()` throughput | 1 MB, 10 MB input; with/without ANSI |
| `benches/dag.rs` | `topological_order()` latency | 10, 50, 100, 200 steps |

### 7.2 Stress tests to implement (`tests/`)

| Test | Description | Pass criterion |
|---|---|---|
| `stress_pty_streams` | Open 20 simulated PTY streams, feed 1 MB/s each, run for 5 s | Frame rate stays >30 Hz, no panic |
| `memory_bounded_after_tab_close` | Open tab, write 100k lines, close tab | `output_lines` length = 0 after drop |
| `task_cancellation_on_tab_close` | Open tab with PTY, close tab, wait 500 ms | All reader/writer threads have exited |
| `workflow_task_cleanup` | Run workflow, stop it mid-execution | No lingering `JoinHandle`s after stop |

### 7.3 Instrumentation recommendation

Add `tracing` spans (if not already present) around:
- `terminal.draw()` call (measure actual render time)
- `tick()` per tab (measure drain time)
- Docker subprocess invocations (measure call latency)

Gate `tokio-console-subscriber` behind the `tokio-console` feature flag. Document how to enable it during development in `aspec/devops/localdev.md`.

---

## Phase 8 – Findings Report and Follow-on Work Items

**Goal:** consolidate all findings into an actionable written report and create follow-on work items.

### 8.1 Findings report structure

Write `aspec/work-items/plans/0033-performance-audit-findings.md` with:

1. **Executive summary** – 3-5 bullet points on the most impactful findings
2. **Per-area findings** – one section per Phase 1–6
3. **Prioritised recommendation table** – all recommendations with priority (High/Medium/Low), estimated impact, and effort estimate
4. **Benchmarks baseline** – numbers recorded during Phase 0/7

### 8.2 Follow-on work items to create

Create one work item per recommendation (using `0000-template.md`). Proposed items based on current codebase analysis:

| Work Item Title | Priority | Phase |
|---|---|---|
| Event-driven render loop (skip draw when no state change) | High | 1.1 |
| Cap `output_lines` with ring buffer (VecDeque, 10k lines) | High | 1.2, 2.1 |
| Wrap blocking Docker calls in `spawn_blocking` | High | 3.1 |
| Audit and fix tab-close task cancellation | High | 4.2 |
| Replace `output_tx` unbounded channel with bounded+lossy | Medium | 2.3 |
| Cache vt100 screen cell spans until PTY data changes | Medium | 1.3 |
| Audit `output_tx` clone sites for post-close retention | Medium | 2.2 |
| Add cancellation tokens to all long-running tab tasks | Medium | 4.1 |
| Make Docker stats poll interval configurable | Low | 3.3 |
| Replace `Mutex<Vec<TuiTabInfo>>` with `RwLock` | Low | 6.3 |
| Investigate `bollard` async Docker client | Low | 5.1 |

Number each work item sequentially from the next available ID at time of filing.

---

## Sequencing and Dependencies

```
Phase 0 (tooling)
  └─► Phase 1 (TUI rendering)  ─────────────────┐
  └─► Phase 2 (memory)         ──────────────────┤
  └─► Phase 3 (CPU)            ──────────────────┤
  └─► Phase 4 (async tasks)    ──────────────────┤
  └─► Phase 5 (Docker)         ──────────────────┤
  └─► Phase 6 (scalability)    ──────────────────┤
                                                  ▼
                                          Phase 7 (benchmarks)
                                                  │
                                                  ▼
                                          Phase 8 (report + work items)
```

Phases 1–6 are independent and can be executed in any order or in parallel if multiple contributors are involved.

---

## Key Files Referenced

| File | Relevance |
|---|---|
| `src/tui/render.rs` | Full-frame draw, scroll computation, vt100 rendering |
| `src/tui/state.rs:301` | `output_lines: Vec<String>` — unbounded output buffer |
| `src/tui/state.rs:543` | `vt100::Parser::new(rows, cols, 1000)` — 1000-line scrollback |
| `src/tui/state.rs:857–991` | `tick()` — channel drain loop |
| `src/tui/pty.rs:62` | `sync_channel(256)` — bounded PTY event channel |
| `src/tui/mod.rs:112` | 16 ms poll timeout (≈60 Hz tick) |
| `src/docker/mod.rs:604` | `run_container_captured()` — blocking `.output()` call |
| `src/docker/mod.rs:681` | `run_container()` — blocking `.status()` call |
| `src/workflow/dag.rs` | Non-memoized topological sort |
| `src/commands/agent.rs` | Docker calls from async context |
