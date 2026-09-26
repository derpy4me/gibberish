---
title: Tactical Message Readability and Unread Indicators - Plan
type: feat
date: 2026-09-26
topic: tactical-message-readability-and-unread-indicators
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

## Goal Capsule

- **Objective**: Humans communicating over the Gibberish mesh can easily distinguish incoming and outgoing airwave messages at a glance and track unread channel and station activity without clutter, cognitive strain, or modern bubble aesthetics.
- **Means**: Full-width monospace terminal rule headers with asymmetric left/right text anchoring in `chat_view.slint`, combined with monospace bracket counters (`[1]`, `[2]`) in `station_roster.slint` that clear immediately upon conversation selection.
- **Product Authority**: Client UI/UX message readability and unread badges own the active scope; OS-level desktop/mobile notification center integrations and audio alerts are contextual candidates and not active scope for this artifact.
- **Open Blockers**: None.

---

## Product Contract

### Summary
A tactical UI/UX enhancement for the Slint mesh client that provides instant visual differentiation between sent and received messages and surfaces unread activity on channels and stations. In the chat feed, messages discard container bubbles in favor of full-width horizontal rule headers (`────────────────────────`) with asymmetric text placement: incoming messages originate on the left with peer-color tags and left-indented body text, while outgoing messages originate on the right with cyan tags, right-anchored body text, and delivery status indicators (`[OK]`, `[*]`). In the sidebar roster, inactive conversations with new messages display monospace bracket counters (`[1]`, `[4]`) that clear instantly upon selection.

### Problem Frame
The initial Slint client implementation renders all messages as flat left-aligned text with identical 2px accent bars, creating a wall of text that makes rapid scanning between local transmissions and remote peer responses difficult. Furthermore, when the user is viewing `#all (Swarm Broadcast)` or a specific direct chat, incoming airwave packets arriving on other channels or peer stations arrive silently with zero visual affordance, requiring manual clicks through the station list to discover missed transmissions.

### Key Decisions
- **Full-Width Terminal Rule Headers with Asymmetric Text Placement**: Full-width horizontal divider lines span 100% of the chat viewport width, eliminating rounded speech bubbles and card containers entirely. Incoming messages anchor to the left with peer-color tags and left-rail rules; outgoing messages anchor to the right with cyan tags, right-rail rules, and status tags (`[OK]`, `[*] QUEUED DTN`). (session-settled: user-directed — chosen over rounded iOS bubbles and left-only card rails: full-width terminal rule dividers with asymmetric left/right text placement directly on the dark canvas). Governs R1, R2, R3, R7.
- **Monospace Bracket Counters (`[N]`)**: Unread activity displays as pure text bracket notation (e.g., `[1]`, `[4]`, `[99+]`) on the right side of channel and station roster items, matching the hacker terminal typography. (session-settled: user-directed — chosen over glowing dots or solid pill badges: minimal bracket notation matching the Slint monospace aesthetic). Governs R4, R6.
- **Immediate Selection-Based Unread Clearing**: Clicking or selecting a channel or station immediately resets its unread counter to zero. (session-settled: user-directed — chosen over scroll-position detection or in-stream unread divider lines: immediate reset on conversation selection). Governs R5.
- **Active Conversation Suppression**: Incoming packets for the currently active channel or station append directly to the open chat feed without incrementing or flashing the sidebar badge. Governs R5.
- **Exclusion of OS Notifications and Audio Pings**: Desktop toast notifications (D-Bus, APNs, Windows Toast) and sound effects are excluded from this scope to keep the client self-contained, air-gap compliant, and dependency-free. Governs Scope Boundaries.

<!-- ce-section: work-relationships -->
### How This Work Fits Together
This plan refines the presentation and interaction layers of the native Slint client established in `gibberish/docs/plans/2026-09-24-1209-feat-slint-mesh-messaging-client-plan.md`.
- **Foundational Client Shell** (Completed):
  - Pure Rust Slint UI shell across desktop and mobile.
  - Pairwise ratcheted DMs and `#all` Swarm broadcast.
  - Station roster telemetry (RSSI, LQI, trust state).
- **Tactical Message Readability & Unread Indicators** (This Plan):
  - Redesign `chat_view.slint` message items into asymmetric full-width rule blocks with pure canvas text.
  - Redesign `station_roster.slint` and client controller state to track and render unread bracket counters (`[N]`).
  - Selection-based unread clearing and active conversation suppression.
- **Subsequent Interaction Capabilities** (Contextual candidates, not active scope):
  - *OS Desktop/Mobile Notifications*: Native system tray / notification center integration when the app is minimized.
  - *Audio Telemetry Pings*: Configurable CW/beeper tone upon packet receipt.
  - *Message Search & Filtering*: Full-text keyword search across SQLite message history.

### Actors
- A1. **Operator**: Human communicating over the 802.15.4 mesh network using the desktop or mobile client.
- A2. **Slint UI View (`ChatView`, `StationRoster`)**: Native graphical interface rendering message feeds, roster items, and unread counters.
- A3. **Client Controller (`controller.rs`)**: Rust backend managing active conversation state, message routing, unread counts, and Slint model updates.
- A4. **Remote Mesh Station**: Distant hardware node transmitting encrypted direct messages or swarm broadcasts over the airwaves.

### Requirements

- R1. The message feed must separate distinct messages using a full-width horizontal rule line spanning 100% of the viewport width.
- R2. Incoming messages must render with header metadata originating on the left (`── [timestamp] <sender> node_id ─────────`) and body text indented along a left accent rule colored by peer identity.
- R3. Outgoing messages must render with header metadata originating on the right (`───────── <ME: node_id> [timestamp] [STATUS] ──`) and body text right-anchored along a cyan right accent rule.
- R4. Message body text must render directly on the terminal canvas without rounded bubble containers, background boxes, or card borders.
- R5. Consecutive message blocks in the chat feed must have at least 14px to 16px of vertical spacing to ensure legible scanning.
- R6. Inactive channels (`#all`) and peer station rows must display an unread bracket counter (`[N]`) when unread incoming messages exist.
- R7. Unread counts exceeding 99 must format as `[99+]` to prevent sidebar horizontal layout displacement.
- R8. Selecting or clicking a channel or station row must immediately clear its unread counter to 0 and remove the bracket badge.
- R9. Incoming messages matching the currently selected conversation must append to the active chat feed without incrementing or displaying an unread badge on that conversation's roster row.
- R10. Outgoing messages sent by the local operator must never increment unread counts.
- R11. Newly discovered peer stations that transmit a direct message while inactive must initialize in the roster with an unread badge of `[1]`.

### Key Flows

- F1. **Incoming Airwave Transmission on Inactive Channel**:
  - **Trigger:** A remote station transmits a packet destined for `#all` or a direct message while the operator is viewing a different station.
  - **Actors:** A3 (Controller), A2 (UI View).
  - **Steps:**
    1. Controller receives incoming message frame from mesh transport.
    2. Controller verifies message conversation ID does not match active conversation.
    3. Controller increments unread counter for that conversation ID.
    4. Slint model updates; `station_roster.slint` renders `[N]` bracket counter in the matching row.
  - **Covered by:** R6, R7, R9, R11.

- F2. **Conversation Selection & Unread Reset**:
  - **Trigger:** Operator clicks an inactive channel or station item displaying an unread badge (`[N]`).
  - **Actors:** A1 (Operator), A2 (UI View), A3 (Controller).
  - **Steps:**
    1. Operator clicks channel or station row.
    2. Callback `select_conversation(id)` fires to Controller.
    3. Controller sets active conversation ID and resets unread counter for `id` to 0.
    4. Controller loads conversation messages into the chat model.
    5. UI removes bracket badge from selected row and renders message feed with full-width rules.
  - **Covered by:** R8.

- F3. **Outgoing Message Transmission**:
  - **Trigger:** Operator types text into input box and presses Enter or clicks SEND.
  - **Actors:** A1 (Operator), A2 (UI View), A3 (Controller).
  - **Steps:**
    1. Operator submits text.
    2. Controller appends message to active conversation with status `[*]`.
    3. Chat feed renders right-anchored rule block with cyan accent and `[*] QUEUED DTN`.
    4. Upon serial delivery confirmation, Controller updates row status to `[OK]`.
  - **Covered by:** R1, R3, R4, R10.

### Acceptance Examples

- AE1. **Visual Distinction Between Sent and Received Messages**:
  - **Given** an active chat view containing both incoming messages from `Alpha` and outgoing messages from `ME: 0xBEBD82B4`.
  - **When** the chat feed renders on screen.
  - **Then** incoming messages are anchored to the left with green accent lines and left-aligned rule headers, outgoing messages are anchored to the right with cyan accent lines and right-aligned rule headers, and no background bubble rectangles are present.
  - **Covers:** R1, R2, R3, R4, R5.

- AE2. **Unread Badge Appears for Inactive Conversation**:
  - **Given** the operator is actively viewing `#all (Swarm Broadcast)`.
  - **When** station `0x4A12F981` (Alpha) sends a direct message to the local node.
  - **Then** the chat view for `#all` does not alter its message list, and the sidebar row for `Alpha` displays `[1]` in amber/cyan bracket notation.
  - **Covers:** R6, R9.

- AE3. **Selecting Unread Conversation Clears Badge**:
  - **Given** station `Alpha` displays `[1]` in the sidebar roster.
  - **When** the operator clicks on `Alpha` in the roster.
  - **Then** the active conversation switches to `Alpha`, the message feed loads `Alpha`'s messages, and the `[1]` badge disappears immediately.
  - **Covers:** R8.

- AE4. **Active Conversation Does Not Accumulate Unread Badge**:
  - **Given** the operator is actively viewing `#all (Swarm Broadcast)`.
  - **When** a new broadcast message arrives from `Bravo-Relay`.
  - **Then** the message appears immediately at the bottom of the chat feed, and the `#all (Swarm)` row in the sidebar does not display any unread badge.
  - **Covers:** R9.

### Scope Boundaries

- **In Scope**:
  - Redesign of `chat_view.slint` message items: full-width terminal rule headers, asymmetric left/right text layout, canvas-direct rendering (zero bubbles), and 14–16px vertical spacing.
  - Redesign of `station_roster.slint`: addition of monospace bracket counters (`[N]`) to `#all` and `StationItem`.
  - Rust controller state tracking in `controller.rs`: maintaining per-conversation unread counters, suppression on active conversation, and immediate clear on selection.
  - High unread count capping (`[99+]`).

- **Out of Scope**:
  - OS-level desktop notification integrations (macOS Notification Center, Linux D-Bus desktop notifications, Windows Toast notifications).
  - Audio/sound effects or buzzer telemetry pings upon message reception.
  - In-feed unread divider lines (`─── UNREAD MESSAGES ───`).
  - Message bubble containers or rounded card backgrounds.

### Success Criteria

- SC1. **Instant Visual Scanning**: Operators can distinguish sent from received messages in under 200ms without reading header sender text, solely through left/right spatial anchoring and accent rules.
- SC2. **Zero Unnoticed Background Messages**: 100% of incoming transmissions on inactive conversations display a visible `[N]` bracket counter in the sidebar within 50ms of receipt.
- SC3. **Immediate Unread State Synchronization**: Clicking an unread channel or station clears its badge in 0 frames (<16ms) without residual unread state.
- SC4. **Minimal Hacker Monospace Integrity**: All new visual elements conform to JetBrains Mono typography and the tactical obsidian color palette with zero rounded modern bubbles.

---

## Planning Contract

### Summary
The technical implementation enriches three layers:
1. `chat_view.slint`: Replaces the single left-aligned row with a 2-part block: a 100% width `HorizontalLayout` containing a `Rectangle` rule line (styled with `#1c2636`) alongside formatted metadata tags (`── [time] <sender> ──`), and a body container that anchors text to the left (with a 2px left border) for incoming messages, or anchors text to the right (with a 2px right border and right alignment) for outgoing messages.
2. `station_roster.slint` & `main_window.slint`: Adds `unread_count: int` to `StationItem` and `in property <int> swarm_unread_count` to `StationRoster` and `MainWindow`. Renders `[N]` or `[99+]` using Slint conditional text bindings.
3. `controller.rs`: Enhances `UiEvent::MessageReceived` to detect non-active conversations and increment `swarm_unread_count` or the respective station row in `stations_model`. Enhances `select_conversation` to zero out the selected conversation's unread counter and update Slint properties immediately.

### Key Technical Decisions
- **KTD1. Slint Structural Layout without Container Wrapping**: To achieve zero bubbles with full-width rules, each message in the `ListView` is structured as a vertical stack: a header `HorizontalLayout` where `horizontal-stretch: 1` stretches a 1px rule line across the available width, and a content `HorizontalLayout` with `alignment: start` or `alignment: end` that holds unboxed text with a 2px identity accent strip.
- **KTD2. Dynamic Unread Formatting in Slint DSL**: Formatting `[N]` and `[99+]` is handled directly in Slint property expressions (`count > 99 ? "[99+]" : ("[" + count + "]")`), avoiding auxiliary string allocations on the Rust controller event loop.
- **KTD3. In-Memory Conversation Unread Map**: The controller maintains unread state directly synchronized with `stations_model` rows and a dedicated `swarm_unread_count` field, keeping Slint data models authoritative and eliminating desynchronization.

### System Architecture & Structural Changes

```
┌────────────────────────────────────────────────────────┐
│                      MainWindow                        │
│ ┌──────────────────────────┐ ┌───────────────────────┐ │
│ │      StationRoster       │ │       ChatView        │ │
│ │                          │ │                       │ │
│ │  #all (Swarm)  [4] <─────┼─┼─ swarm_unread_count   │ │
│ │  Alpha         [1] <─────┼─┼─ StationItem.unread   │ │
│ │  Bravo-Relay   [2]       │ │                       │ │
│ └──────────────────────────┘ │ ┌───────────────────┐ │ │
│                              │ │ Incoming: Left    │ │ │
│                              │ │ ── <Alpha> ─────  │ │ │
│                              │ │ │ Msg text        │ │ │
│                              │ │                   │ │ │
│                              │ │ Outgoing: Right   │ │ │
│                              │ │ ───── <ME> [OK] ──│ │ │
│                              │ │          Msg text │ │ │
│                              │ └───────────────────┘ │ │
│                              └───────────────────────┘ │
└───────────────────────────▲────────────────────────────┘
                            │ (Events & Callbacks)
┌───────────────────────────┴────────────────────────────┐
│                    SlintController                     │
│  - UiEvent::MessageReceived: increment if not active   │
│  - select_conversation: reset target unread count to 0 │
└────────────────────────────────────────────────────────┘
```

### Technical Dependencies
- Slint GUI toolkit version 1.15.0+ (already in `gibberish/apps/gibberish-client/Cargo.toml`).
- Zero new external crates or system library dependencies.

### Assumptions
- The native Slint compiler correctly supports `wrap: word-wrap` and layout alignment inside `ListView` items across desktop and mobile targets.
- Message row heights in Slint dynamically fit multiline text when using vertical layouts with inner text wrapping.

---

## Implementation Units

### U1. Redesign Chat View with Full-Width Rules and Asymmetric Layout
- **Goal:** Transform `chat_view.slint` message items from flat left-rail rows into tactical, bubbleless, asymmetric blocks with 100% extended rule headers.
- **Touched Files:**
  - `apps/gibberish-client/ui/chat_view.slint`
- **Requirements Covered:** R1, R2, R3, R4, R5
- **Implementation Steps:**
  1. In `chat_view.slint`, update each message item in `ListView`:
     - Wrap in a `VerticalLayout` with `spacing: 4px; padding-top: 6px; padding-bottom: 6px;`.
     - Implement the top rule header:
       - If `!msg.is_outgoing`: Left metadata (`"── [" + msg.timestamp + "] <" + msg.sender + ">"`), then a 1px rule line `Rectangle { height: 1px; background: #1c2636; horizontal-stretch: 1; }`.
       - If `msg.is_outgoing`: 1px rule line `Rectangle { height: 1px; background: #1c2636; horizontal-stretch: 1; }`, then Right metadata (`"<" + msg.sender + "> [" + msg.timestamp + "] " + msg.status + " ──"`).
     - Implement the message body row directly below the header:
       - If `!msg.is_outgoing`: Left-aligned with 2px accent strip colored by `msg.sender_color`, padding-left 8px, and pure text on canvas with `wrap: word-wrap`.
       - If `msg.is_outgoing`: Right-aligned with padding-right 8px, pure text on canvas with `wrap: word-wrap`, and 2px accent strip colored `#38bdf8` on the right edge.
  2. Verify vertical breathing room between message items achieves 14–16px total separation.
- **Verification:** Run `cargo check -p gibberish-client` and visual snapshot inspection.

### U2. Add Monospace Bracket Counters to Station Roster & Main Window
- **Goal:** Support unread message counter badges on channels and station rows in Slint UI.
- **Touched Files:**
  - `apps/gibberish-client/ui/station_roster.slint`
  - `apps/gibberish-client/ui/main_window.slint`
- **Requirements Covered:** R6, R7
- **Implementation Steps:**
  1. In `station_roster.slint`:
     - Add `unread_count: int` field to `export struct StationItem`.
     - Add `in property <int> swarm_unread_count: 0;` to `StationRoster`.
     - In `#all` channel button `HorizontalLayout`, add unread bracket counter text:
       ```slint
       if root.swarm_unread_count > 0 : Text {
           text: root.swarm_unread_count > 99 ? "[99+]" : ("[" + root.swarm_unread_count + "]");
           color: #10b981;
           font-size: 10px;
           font-weight: 700;
           vertical-alignment: center;
       }
       ```
     - In the `stations` list item top row `HorizontalLayout`, add station unread bracket counter text:
       ```slint
       if station.unread_count > 0 : Text {
           text: station.unread_count > 99 ? "[99+]" : ("[" + station.unread_count + "]");
           color: station.trust_state == "verified" ? #38bdf8 : #f59e0b;
           font-size: 10px;
           font-weight: 700;
           vertical-alignment: center;
       }
       ```
  2. In `main_window.slint`:
     - Add `in property <int> swarm_unread_count: 0;` to `MainWindow`.
     - Forward `swarm_unread_count: root.swarm_unread_count;` into `StationRoster`.
- **Verification:** Run `cargo check -p gibberish-client` to confirm Slint compilation succeeds.

### U3. Controller Unread Counter Management and Conversation Switch Clearing
- **Goal:** Track incoming message counts for inactive conversations, suppress increments for the active conversation, and reset counts to 0 upon selection.
- **Touched Files:**
  - `apps/gibberish-client/src/controller.rs`
- **Requirements Covered:** R8, R9, R10, R11
- **Implementation Steps:**
  1. In `controller.rs`, update `UiEvent::StationDiscovered` to initialize `unread_count: 0`.
  2. In `controller.rs`, update `UiEvent::MessageReceived(msg)`:
     - Check `active_convo = ui.get_active_convo_id();`
     - If `convo_id == active_convo`: append message to `messages_model` as before (no unread increment).
     - If `convo_id != active_convo` and `!msg.is_outgoing`:
       - If `convo_id == "#all"`:
         - `ui.set_swarm_unread_count(ui.get_swarm_unread_count() + 1);`
       - If `convo_id != "#all"`:
         - Find matching station row in `stations_model`. If found, increment `row.unread_count += 1` and update row data.
         - If station is not yet in `stations_model`, push a new `StationItem` with `unread_count: 1`.
  3. In `select_conversation(id)` callback handler:
     - If `id == "#all"`: `ui.set_swarm_unread_count(0);`
     - If `id != "#all"`: search `stations_model` for `row.node_id == id`, set `row.unread_count = 0`, and `set_row_data`.
- **Verification:** Run `cargo test -p gibberish-client` to verify existing tests compile and pass.

### U4. Controller and UI Regression Tests
- **Goal:** Author automated unit tests validating unread counter incrementing, active view suppression, and selection clearing.
- **Touched Files:**
  - `apps/gibberish-client/tests/controller_test.rs`
  - `apps/gibberish-client/tests/visual_snapshot_test.rs`
- **Requirements Covered:** R1-R11, AE1-AE4
- **Implementation Steps:**
  1. In `controller_test.rs`, add tests:
     - `test_unread_counter_increments_for_inactive_conversation()`: Emit message for `#all` while viewing a station; assert `swarm_unread_count == 1`.
     - `test_unread_counter_suppression_for_active_conversation()`: Emit message for active conversation; assert unread count remains 0.
     - `test_unread_counter_clears_on_conversation_selection()`: Emit message to station; assert `unread_count == 1`; trigger `select_conversation`; assert `unread_count == 0`.
  2. In `visual_snapshot_test.rs`, update mock data structs with `unread_count` and verify visual rendering renders clean layout.
- **Verification:** Execute `cargo test -p gibberish-client`.

---

## Verification Contract

### Automated Verification
```bash
# Verify Slint and Rust compilation
cargo check -p gibberish-client --tests

# Run unit tests covering controller and unread logic
cargo test -p gibberish-client --test controller_test

# Run full client test suite including visual snapshot tests
cargo test -p gibberish-client
```

### Manual Verification Scenarios
1. **Asymmetric Visual Check**: Launch client (`cargo run -p gibberish-client`). Send a local message; observe right-anchored block with cyan accent and `[OK]`. Receive peer message; observe left-anchored block with peer color.
2. **Full-Width Rule Check**: Resize window; observe rule line dynamically stretches across the full chat pane width.
3. **Unread Counter Notification Check**: Switch to station `Alpha`. Inject swarm broadcast message; observe `:: #all (Swarm) ::` receives `[1]` badge in emerald.
4. **Immediate Clear Check**: Click `:: #all (Swarm) ::`; observe `[1]` badge disappears instantly.

---

## Definition of Done

- [ ] All requirements R1 through R11 are implemented and verified.
- [ ] `chat_view.slint` renders full-width rule dividers with asymmetric sides and no rounded bubbles.
- [ ] `station_roster.slint` displays monospace bracket counters (`[1]`, `[4]`, `[99+]`) for unread messages.
- [ ] Selecting an unread conversation immediately clears its counter.
- [ ] All client unit tests pass with zero regressions: `cargo test -p gibberish-client`.
- [ ] Memory footprint remains under 15MB with instant startup (<50ms).
