# Work Item: Task

Title: Performance Audit
Issue: issuelink

## Summary:

Perform a comprehensive performance audit of amux to identify bottlenecks and inefficiencies across memory usage, TUI rendering, CPU-intensive operations, async task scheduling, and Docker container management. The output of this work item is a set of concrete, prioritized recommendations — not code changes. Actual fixes will be tracked as follow-on work items.

## User Stories

### User Story 1:
As a: user

I want to:
run amux with many concurrent tabs open without experiencing UI lag or sluggishness

So I can:
stay focused on my work without being slowed down by the tool itself

### User Story 2:
As a: user

I want to:
trust that amux uses minimal CPU and memory while idle or running background tasks

So I can:
run amux alongside other resource-intensive workloads (builds, tests, other containers) without contention

### User Story 3:
As a: user

I want to:
see fast, responsive terminal output in each tab even when multiple containers are streaming output simultaneously

So I can:
monitor multiple agents in parallel without scrolling lag or dropped output


## Implementation Details:

This work item covers investigation and documentation only. For each area below, the audit should identify the current approach, measure or estimate its cost, and produce a recommendation with a priority rating (high/medium/low).

### 1. TUI Rendering Efficiency (`src/tui/`)

- Audit `render.rs` to determine whether full-frame redraws occur on every tick or only on dirty regions/state changes.
- Identify whether the render loop is driven by a fixed-interval tick or by events (input, PTY output, async task completion). A pure-tick approach wastes CPU during idle periods.
- Check how large terminal outputs (e.g. thousands of lines of container logs) are handled — are they buffered, truncated, or stored in full? Assess memory growth over long-lived sessions.
- Investigate whether Ratatui widgets allocate on every frame or reuse allocations.
- Review the scroll buffer implementation in `state.rs`: is there a maximum retained line count, and is scrollback capped to prevent unbounded growth?

### 2. Memory Usage

- Profile heap allocations across a typical session: startup, opening tabs, receiving PTY output, and closing tabs.
- Check for retained output buffers that are never freed after a tab is closed.
- Identify any `Arc`/`Rc` cycles or `clone`-heavy paths in state management (`state.rs`).
- Assess whether Docker log streams are buffered entirely in memory or streamed and discarded.
- Look for large `String` or `Vec<u8>` allocations that could be replaced with bounded ring buffers.

### 3. CPU-Intensive Operations

- Locate any synchronous, blocking operations on the async executor (e.g. `std::thread::sleep`, blocking I/O, or heavy computation on a Tokio task without `spawn_blocking`).
- Review ANSI/VT escape sequence parsing in `pty.rs`: evaluate whether parsing is done incrementally or in bulk, and whether it dominates CPU during high-throughput output.
- Check whether workflow DAG evaluation (`workflow/dag.rs`) involves repeated recomputation that could be memoized.
- Identify any polling loops with short sleep intervals that could be replaced with async notifications or wakers.

### 4. Background Async Task Efficiency (`src/commands/`, `src/workflow/`)

- Map all long-running async tasks spawned during a session (container watches, status polling, output streaming).
- Verify that tasks are cancelled and cleaned up when tabs or workflows are stopped — look for orphaned `JoinHandle`s or undriven futures.
- Evaluate Docker event polling frequency: is the interval configurable, and is it appropriate for the typical use case?
- Check whether multiple tabs monitoring the same container duplicate subscriptions or share a single stream.
- Review channel usage (e.g. `tokio::sync::mpsc`) for unbounded channels that could grow under backpressure.

### 5. Docker Container Management Performance (`src/docker/`)

- Audit `mod.rs` for the number of Docker API calls made per operation (start, stop, status check) and whether any can be batched or cached.
- Check whether Docker client instances are reused across calls or re-created each time.
- Assess container startup latency: identify any unnecessary sequential steps that could be parallelised.
- Review how container output is streamed: is it read in small chunks (high syscall overhead) or with appropriate buffer sizes?
- Evaluate cleanup logic: are stopped containers removed promptly, and does cleanup block the UI?

### 6. Scalability with Many Concurrent Tabs

- Define "many tabs" — establish a target (e.g. 10, 20, 50 concurrent containers) and assess whether there are O(n) render or polling paths that degrade at scale.
- Identify any shared locks (e.g. `Mutex`, `RwLock`) that become contention points as tab count grows.
- Assess whether the TUI render time grows linearly with tab count and if inactive tabs can be rendered lazily.


## Edge Case Considerations:

- **Very long-running sessions**: memory buffers for PTY output may grow indefinitely; audit needs to establish whether there is a cap and what happens when it is reached.
- **Rapid tab open/close cycling**: verify that resources (channels, tasks, Docker connections) are fully released and not leaked.
- **High-throughput container output**: a container emitting megabytes per second of logs should not starve other tabs or block the render loop.
- **Containers that exit immediately**: ensure cleanup paths are exercised and do not leave zombie tasks.
- **Reconnection after Docker daemon restart**: async tasks watching a gone daemon should fail gracefully without spinning.
- **Very wide/tall terminals**: Ratatui layout computation may be more expensive at extreme sizes; check for quadratic layout passes.
- **Low-resource environments**: amux may be used inside a CI container with limited CPU/RAM; the audit should flag any assumptions of abundant resources.


## Test Considerations:

- Establish baseline benchmarks (using `cargo bench` / `criterion`) for: render frame time at N tabs, PTY parse throughput, and Docker API call latency before and after any recommended changes are implemented.
- Write a stress test that opens 20+ simulated PTY streams and measures frame rate degradation.
- Add a memory snapshot test that verifies output buffer size is bounded after a tab is closed.
- Confirm with integration tests that async tasks are fully cancelled when a workflow is stopped — no lingering tasks after teardown.
- Use `tokio-console` or `tracing` instrumentation to visualise task lifetimes during the audit; recommend retaining this instrumentation in debug builds.


## Codebase Integration:

- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- Key files to review: `src/tui/render.rs`, `src/tui/state.rs`, `src/tui/pty.rs`, `src/docker/mod.rs`, `src/workflow/dag.rs`, `src/commands/agent.rs`.
- Any profiling tooling (e.g. `cargo flamegraph`, `heaptrack`, `tokio-console`) should be added as optional dev dependencies only and must not affect the release binary size or behaviour.
- Recommendations should be documented as follow-on work items using `aspec/work-items/0000-template.md`, one per distinct concern, so they can be prioritised and scheduled independently.
- Do not introduce any new runtime dependencies as part of the audit itself.
